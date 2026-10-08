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

**One `edit` for every application** *(the user, 2026-10-05, planning `X5`:
the `edit` API has to stay consistent across all the applications)*. Each
application is reached through the same verb with the same contract, and an
application that needs to depart from it changes the contract for all of them
or does not depart:

- `edit(structure, *, sample_rate, host, open, **options)` in Python and
  `edit(structure, options)` in TypeScript, the editor chosen by **what the
  structure holds**, never by the class that built it.
- It **returns the editor**, open unless `open` is false: the handle the window
  is addressed by (`close`, `on_closed`, `undo`/`redo`). Never the data.
- **The edited data is read on the structure passed in**, which is the edited
  one. The two exceptions are stated in `edit`'s own docstring and are part of
  the contract: a buffer, which the audio editor edits as a copy and writes
  back on `save`, and a timeline, which is code and is rendered first, its
  events being the editor's `sequence`.
- **The options common to every editor** (`title`, `width`, `height`,
  `base_id`, `extra`, `context`) mean the same in all of them; a domain's own
  option (`min`/`max`/`start`/`end` for a curve, `y_axis` for a roll, `until`
  for a timeline) is named once and documented with `edit`.
- **Where one structure has several presentations** (a sequence as a roll or
  as a score), `view` chooses which and the structure gives the default; it is
  one option for every structure that has more than one, not a name per
  application.

A new application closes only when `edit` reaches it under this contract in
both clients, with its row in `edit`'s docstring and TSDoc.

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
  keeps and nothing reads, and no client sends one. What
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

  **The question it opened is the one `O26` left:** the events view is shaped out
  of the **client's own objects** — a `Timeline` of `OscItem`/`MidiItem`, read
  through the client's `_pitch`, `_velocity` and `_label_of` — which is why it did
  not move with the other projections (`crates/clausters-document/PLAN.md`, `O26`).
  **Settled with the user 2026-09-27**, as follows.

  **The principle: a roll is a plane of events, time on X and a domain on Y.** It
  behaves as the multitrack does, and what changes is the domain of the data: a
  note is a region (a start, a length, an identity of its own); the Y axis is a
  *value* of the event where the multitrack's is a containment (the track); a
  track's automation is the roll's CC, bend and pressure lanes; a region's
  automation is a note's own expression (MPE), drawn inside the box; a region's
  gain is the velocity, drawn **inside the rectangle** (the velocity lane goes);
  a source several regions point at is an `EventSequence` several regions share.
  What is shared is the gesture machine, not the element: what `(dx, dy)` means
  is the domain's, so the `multitrack` and `notes` elements stay two.

  **The Y domain is data**: the key it reads and writes, a scale (linear, log,
  categorical), a ruler, a grid and a quantum. MIDI note first; then **Hz** — the
  same magnitude in another coordinate, drawn on the log scale the spectrogram
  already has (`freq_scale`, `display_to_hz`), the "analog score" whose precedent
  is Xenakis's UPIC; then n-TET, a drum map, any numeric control. An expression
  whose target is the Y key is drawn in the plane as a trajectory (a glissando in
  Hz, an MPE per-note bend); the others go normalized inside the box. Events
  without the Y key go to another lane (raw OSC as marks, CC as curves).

  **Decisions:**

  - **What the roll edits is the document's, in Rust**, with an id per event:
    the **`EventSequence`**. A `Timeline`, a `.mid`, a recording and the
    multitrack's notes region all open into it.
  - **The editor edits the rendered object, with no copy.** The client object is
    a handle over the Rust structure; there is no mirror and no write-back onto
    the source `Timeline`, and the editor sounds with **its own playback**.
  - **A `Timeline` has two states that coexist and are not reconciled.** Before
    it plays it is client code (playables, generators, nesting, a tempo curve);
    after, concrete data. **`render_events`** is the one-way change between them
    (`timeline.render_events(...)`, `pattern.render_events(...)`): no timeline is
    rebuilt from its values, no callable from its result. So **`edit(timeline)`
    changes behaviour**: it renders the timeline and opens the sequence — said in
    the books and in `edit_notes`.
  - **An event becomes concrete data when it plays**: a client object becomes a
    data value (a node, a buffer number). The time is not lost: the data keeps
    its times in **beats** and carries the flattened tempo map, as a `.mid` does.
  - **`Event` and its semantics go to `clausters-core`**, and the client `Event`
    stays a dictionary that calls them: key families (pitch `freq` / `midinote`
    / `degree` + `alter` + `scale` + `octave` + `root`; level `amp` / `velocity`
    / `db`; length `sustain` / `dur` / `legato` / `stretch`; MIDI `channel`,
    `program`; the notation keys), **coherence between keys** (editing `freq`
    updates `midinote` and back), a `type` (`note`, `rest`, `midi` with
    `midicmd`, `osc`), and a render per destination (`/synth_new` and its release;
    MIDI messages; an OSC message to another application).
  - **`MidiItem` and `OscItem` become `Event`s of their `type`**, not a class
    hierarchy — what the roll edits and what a rendered timeline holds.
  - **A degree is altered by its own key, `alter`**, in semitones (real, for
    microtones; 0 by default; reserved, never sent to a synth) — MusicXML's name,
    since `accidental` is already notation's (the courtesy sign). Core gets
    `degree_to_midinote(degree, alter, octave, root, scale)` and its inverse
    `midinote_to_degree(midinote, octave, root, scale, spelling) -> (degree,
    alter)`, which coherence uses when the roll moves a note written by degree;
    `spelling` picks sharp or flat. A fractional SuperCollider-style degree and a
    `(degree, alter)` tuple are accepted as input and normalized to the two
    keys, so pattern arithmetic stays on integers. Today's `degree_to_midinote`
    drops the fraction and truncates negatives toward zero; that is fixed here.

  **The conversions pivot on `Event`:** `Timeline` → sequence (rendering, one
  way, in the clients over the Rust type); `.mid` both ways in `clausters-midi`
  (which only writes today: note on/off pairing, CC to curves, tempo); live MIDI
  through a recorder in `clausters-midi`; `Score` ↔ sequence through
  `notation::interp::Note`, which already carries `pitch`, `amp`, `sustain`,
  `spelling` and `marks`; and Event → OSC for a synth in the core. `MidiScore`
  and `OscScore` stay what is compiled from a sequence for a file or an offline
  render, not what is edited.

  **The steps**, each closing with its commit, in this order because each is
  written on the one before:

  - ✅ **X3.0 - Before the editor.** The float velocity the host read as 100
    ("A roll's edit sends every note at its velocity's amplitude", Found by use),
    and a timeline on a server transport hearing an edit (`C54`). `C54` was
    closed 2026-10-03 without its by-ear pass, and its edit half removed.
  - ✅ **X3.1 - `Event` in the core**: families, coherence, `alter`, `type`,
    render per destination; the C and wasm doors; both clients delegate and
    delete their own derivation (`midinote()`, `freq()`, `sustain()`, velocity ↔
    amp exist twice today, `seq/event.py`, `seq/event.ts`); parity vectors.
    *(Shipped 2026-09-27: `clausters_core::event` and `event::render`. The
    synth's messages, the MIDI messages and a MIDI message read back as an
    event are the core's; `OscItem` and `MidiItem` are events of type `"osc"`
    and `"midi"`, made by a function of that name in both clients, and a
    document's older `{"osc": ...}` / `{"midi": ...}` spelling still reads. The
    editing crate's intake writes a moved note through the core's coherence, so
    a note written with `freq` or by degree follows the drag.)*
  - ✅ **X3.2 - `EventSequence` in the document**, replacing today's
    `events::Events` (opaque `{at, data}`, identity by position): events with
    ids, beats and the tempo map, curve lanes with the existing `Automation` on
    a beat axis, a note's expression as `Automation` relative to its start (so
    M35 is provided for), unknown keys kept, a `Session` source a region can
    point at. The handle in both clients: a playable, written to `.mid`, opened
    by `edit`.
    *(Shipped 2026-09-27: `clausters_document::events::EventSequence`, its
    id-naming vocabulary (`add`, `remove`, `move`, `set` with the core's
    coherence, `keys`, `setevents`, `tempo`, and `restore` as every edit's
    inverse), `Location::Events` (session format 4), and
    `clausters.seq.EventSequence` / `seq.EventSequence` over one JSON door. A
    whole list without ids still reads and takes the ids of the events it
    matches, so today's roll keeps working until X3.5. Playing a sequence,
    writing it to `.mid` and opening it with `edit` are X3.3, X3.7 and X3.8.)*
  - ✅ **X3.3 - The conversions**: `render_events`; `.mid` (write from the
    sequence; read); `Score` ↔ sequence.
    *(Shipped 2026-09-27: `Timeline.render_events` and
    `EventPattern.render_events` play offline against a destination that
    records, in the root's beats with the timeline's map; `to_smf` /
    `from_smf` over `clausters-midi`'s new tempo writer and reader, the pairing
    of note-ons with their note-offs in the core; `notation.to_sequence`, and
    `sheet_from_timeline` reads a sequence. A tempo ramp goes to a file as the
    step at its breakpoint. `renderEvents` is asynchronous in the web client,
    whose offline session is.)*
  - ✅ **X3.4 - The shared conversation turn** ("Each editor writes the
    conversation's turn again", Found by use): the notes editor would be its
    third copy.
  - ✅ **X3.5 - `clausters_editing`**: the projection and the intake with ids,
    and the Y domain as data.
    *(Shipped 2026-09-27: `clausters_editing::notes` -- `YDomain` (key, scale,
    ruler, window, quantum; `midi()` and `hz()`), `project` (quintuples, a
    parallel `note_ids`, the marker lane) and `intake` (sextuples with the id
    first, each changed key written through the core's coherence, id 0 a new
    note). The whole-list `events` intake stays for today's roll until X3.7
    deletes it.)*
  - ✅ **X3.6 - The `notes` element**: ids on the wire (the end of identity by
    order: deleting note *k* hands note *k+1* the data of *k*), the velocity
    inside the note, the axis with a domain (MIDI note).
    *(Shipped 2026-09-27: `note_ids` beside `notes`, a report of sextuples
    with the id first when a roll has them, a copied note (a split's second
    half, a paste) reported as new; the velocity drawn as the note's fill and
    set with Shift and a vertical drag, and the velocity lane gone, with its
    `velocity` option in both clients' `pianoroll`. The axis stays MIDI notes,
    which is the domain the host already drew; a domain the host reads
    arrives with the Hz one, X3.12.)*
  - ✅ **X3.7 - `clausters-apps::notes`**: the window, the conversation with the
    edit vocabulary by id (move through the domain, trim, split, join, quantize,
    transpose, level, add, delete, duplicate), a member of `Editing`, the doors,
    both clients as handles, the standalone host. `edit(timeline)` with its new
    behaviour, the books, `edit_notes` rewritten. `NotesDomain`, `NotesView`,
    `NotesEditor`, the whole-list intake and each client's note reading
    (`_pitch`, `_length`, `_velocity`) are deleted.
    *(Shipped 2026-09-28: `NotesEditor` a member of `Editing`
    (`Member::Notes`, `openNotes`, `Effect::Notes`), editing the sequence it
    shares with the client's handle through `Arc<Mutex>` -- opened over the
    handle by `clausters_apps_editing_open_notes` / `EditingCore.openNotes`;
    both clients' `NotesEditor` a thin handle, `edit(timeline)` rendering
    first, and `edit_notes` rewritten in both. The vocabulary by id is the
    roll's gestures through `notes::intake` (move, trim, level, add, delete,
    and split/join/quantize as the roll's own keys report them); a transpose
    or a duplicate as a verb of the editor waits for a use. The standalone
    host reaches the editor with X3.9, where a notes region opens it: the
    `--session` host opens only the multitrack today, the audio editor
    included.)*
  - ✅ **X3.8 - Its own playback**: a transport of its own (as
    `AUDIO_EDITOR_TRANSPORT`), the Event → OSC render in Rust on
    `/sched_atTransport`, an edit re-planned while it sounds (`/sched_clear
    "transport" <id>` already exists), and a MIDI destination.
    *(Shipped 2026-09-28: `clausters_editing::notes_playback`, on
    `NOTES_EDITOR_TRANSPORT` (3), its doors, and both clients' `play`,
    `pause`, `resume`, `stop` and `playing` on the editor, the space bar
    included -- the window says `plays`. A replan after every edit and every
    step keeps the releases of what is sounding. The caller hands in the
    transport's clock, since steps cannot read a reply. A MIDI destination
    plays through the client's clock (`play(destination=MidiServer)`), the
    server having no MIDI output; an edit is heard there from the next play.
    A blob in a step that is not samples -- a bundle -- now travels as hex.)*
  - ✅ **X3.9 - The notes region in the multitrack**: an `EventSequence` source
    through `Content::Window`, drawn in the box and edited in the roll opened on
    it (unlike audio, `X1.10`, here the two interoperate), sounding on the
    multitrack's transport; a source shared by several regions changes in all,
    and making it unique is the pending clone verb (`clients/python/PLAN.md`,
    Future directions).
    *(Drawn and edited 2026-09-28: a region over a sequence source draws the
    notes its window reads, a notes editor over the same handle edits what it
    draws, and a session's table holds one (`Source.events`). They sound from
    one event lane on the multitrack's transport (`PLAN.md`, `T8`), outside
    the tracks' strips: a note's own `out`, with the track's mute and solo and
    the box's mute deciding what is placed.)*
    **Left, before the tick** *(listed 2026-09-28, all done the same day)*:
    - ✅ **The examples show a notes region**, in the ones that already exist
      rather than a new one (the user: an application's parts go in its own
      examples, or they scatter): `edit_multitrack` gains a track of notes, an
      `EventSequence` among its `sources`, drawn in its box and sounding from
      the transport's lane, in both clients with the same calls.
    - ✅ **The roll and the box over one sequence, by hand**: the same example
      opens the roll by a double click on the keys box (below), in the
      multitrack's context, so an edit in the roll redraws the box and is
      heard from the lane. Tested in the crate, the drawing, both clients and
      the host; the pass through real windows is the owed ear and eye check.
    - ✅ **The session keeps it**: `edit_multitrack` saves the sequence with
      `Source.events`, and `load_multitrack` reopens it with
      `{**session.load(), **session.sequences()}`, in both clients.
    - ✅ **The standalone host** (`clausters-gui --session`) binds the
      session's sequences: its owner holds one handle per sequence, the
      multitrack editor draws them, the playback's lane plays them, a save
      writes them back, and a double click opens the roll in a window of its
      own (the host leaves the window as an effect its front opens,
      `Host::take_effects`).
    - ✅ **Decision: does a double click on a notes box open the roll?**
      *Yes* (the user, 2026-09-28: "doble clic abre roll"). The one
      exception to `X1.10`, which still holds for a box of samples: the host
      reports `open <box>` for a box that draws a roll, the multitrack editor
      answers `Outcome::open` with the source when it is a bound sequence,
      and each client's editor (`open_roll` / `openRoll`) and the standalone
      host open a notes editor over it in the multitrack's context.
  - ⬜ **X3.10 - The notes editor records, through `PLAN.md` `T10`.**
    Recording MIDI is the server's (moved to `T10` by the user, 2026-09-29:
    it is a server feature, not the editor's). What stays here is the
    editor's side: arming a recording from the roll, and opening the take the
    server wrote as the roll's source. Taken after `T10`; both are skipped
    for now, with `T9` and `X3.11` first.
  - ✅ **X3.11 - CC lanes and per-note expression**, over `PLAN.md` `M35`.
    *(Designed with the user 2026-09-29.)* The data is there since X3.2 --
    `EventSequence.lanes` (curves over the sequence: CC, bend, pressure) and
    `Event.expression` (curves over one note, from its start), both
    `Automation`. What is missing is editing them and hearing them.

    **Two scopes, as MIDI 2.0 has them and as the multitrack already does.**
    *(Redesigned with the user 2026-09-30, replacing "heard by the event's
    type": every note of a sequence is `type: note` -- a `.mid` is paired into
    notes too -- so the type separates nothing.)* A control acts on a whole
    channel or on one note, and that is the multitrack's two places for one
    `Automation`: a sequence's **lane** is a track's automation (a function
    over time, and a note sounding reads it during its span), a note's
    **expression** is a region's (the note's own envelope, from its start). A
    note is to a roll what a region is to a multitrack, and the host already
    draws a note's curves through the clip's own `track::clip_local_view`.
    The user's framing: a note built from outside functions and a note that
    carries its own envelopes always coexist, so what the roll needs is the
    mechanism that goes from one to the other, and a roll that draws MIDI
    must draw what MIDI does -- a channel function as a lane, a per-note one
    in the note -- or it misleads; the server's resources follow the same
    abstraction. Four decisions, taken with the user:
    - **A lane has a channel.** Its target names the MIDI channel it is on
      (none: every channel), and it acts only on that channel's notes -- in
      MIDI the channel is part of a function's address.
    - **Combining the two scopes on one control**, per dimension: a bend (in
      semitones) is the channel's plus the note's, as MIDI 2.0's per-note
      bend and MPE's master channel add; any other control takes the note's
      expression while the note has one, else the lane.
    - **An expression may run past the note-off**, into the release: a note
      does not end at its off as a region ends at its end, it changes its gate
      (and in MPE its member channel still reaches it). **The note's box stays
      `at` + `dur`**, the span between its on and its off: it draws the
      message, not what sounds (the user, 2026-09-30); only the expression's
      layer may extend past it.
    - **The roll only offers what the destination can say.** Per note, plain
      MIDI 1.0 has the attack and release velocity and poly pressure and
      nothing else; MPE adds bend, pressure and timbre (each a member
      channel's message); MIDI 2.0 its per-note bend and controllers; a note
      for the server any control. A per-note curve the sequence's destination
      cannot play is not offered, rather than drawn and not heard.

    **The conversions**, the part that is missing entirely:
    - **Messages into curves**, reading: a channel's stream of CC, bend or
      channel pressure becomes a lane of step points (MIDI holds the last
      value); poly pressure and an MPE member channel's stream become the
      expression of the note on that key or channel. Only the discrete
      gestures stay events -- a program change, a sysex, raw bytes. Today
      `render::from_midi_messages` keeps every such message a `midi` event,
      which the roll draws as a marker: a function drawn as a list of labels.
    - **Curves into messages**, writing or playing MIDI: a lane as sampled
      channel messages; an expression as poly pressure or as MPE (a member
      channel per note), else refused.
    - **Channel to note**: a lane's span copied into each note's expression.
      Nothing of the note is lost; that the curve was shared is.
    - **Note to channel**: exact only while the notes do not overlap, so
      refused (or confirmed) under polyphony.

    **Heard on the server, as graphs**, by scope -- the user's direction
    (2026-09-30): a note's nodes are designed as a GraphDef per note event, as
    the multitrack and the audio editor were, applied to the events' domain. A
    **channel** is a graph instance (a track, in the multitrack's terms): its
    shared members read the channel's curves on the transport's position onto
    private control buses, shared as the channel is. A **note** is a slot of it
    (a clip): a nested graph holding the note's def beside the readers of its
    own curves on buses of its own and, where a bend is in either scope, a
    node making its pitch from the sum. Decided with the user: an instance
    **per channel**, a slot **per shape** of note (the def, the controls it
    starts with, its own curves, the channel's -- a slot's members are fixed),
    the graphs generated and named by what they hold as the multitrack's are
    by width. The voice is marked in its graph as the member the graph
    **ends** with, rather than written with a done action that frees its
    group: the def is the user's and is played outside a graph too, and a
    done action frees only the nearest group, which would leave the slot
    around the note's graph. The tabulation is the multitrack's
    (`multitrack::nodes::curves`, a curve's points on the frame axis from an
    origin), grown to take a local-to-frame map so a note's beats through the
    tempo map use it too -- one function for a region, a track, a lane and a
    note.

    **The steps**, in this order:
    - ✅ **X3.11a - The lanes and the expression in the editor.** CC lanes
      under the roll, drawn and edited as a multitrack's automation rows; a
      note's expression inside its box; its bend drawn in the plane as a
      trajectory (the pitch axis already draws a note's bend line).
      *(Shipped 2026-09-30: the sequence's vocabulary grew `lane`,
      `removelane`, `expression` and `removeexpression`, curves drawing ids
      off the events' counter; the crate projects `curves`, `layers` and
      `points` and reads a `points` report as the one curve it changed; the
      host's roll draws the lanes as rows under its plane and each note's
      curves as layers over it -- a bend over the pitches its range spans --
      all `curve` bodies; both clients' `EventSequence` add and remove them
      (`add_lane`/`addLane`, `add_expression`/`addExpression`), and both
      `pianoroll` builders take the three props.)*
    - ✅ **X3.11b - The scopes in the editor.** A lane's channel; an
      expression's layer past its note's off, the box unchanged. *(Shipped
      2026-09-30: a lane's target takes `channel`, labelled on its row
      counted from 1; a note's layer reaches its last point and a hand
      dragging that point lengthens it, the box drawn from the note's on and
      off as before. The per-note curves offered by the destination moved to
      X3.11d by the user's choice: the editor plays only on the server today,
      where every per-note control is legal, so there is nothing to restrict
      until it has a MIDI destination.)*
    - ✅ **X3.11c - Heard.** The shared tabulation; a lane's reader and bus
      mapped by its channel's notes, an expression's reader per note, the bend
      summed -- in the crate's notes playback and the multitrack's notes
      regions. *(Shipped 2026-09-30: `nodes::tabulate` and `value_at` are
      public and the multitrack's curves go through them; the core's
      `event_graph` writes the channel and note graphs and `ev.pitch`, and the
      editing crate's `note_curves` plans which notes play in which graph,
      samples every table and keeps the instances and buffers, a table a
      sounding note may read given back one plan later. The server grew a
      GraphDef member's `ends` and a lane note whose voice is a graph's slot
      (`{"graph": id, "slot": name}`, fired as `/graph_addSlot`). Placed
      events carry an id, a scope and their curves, and a placement its lanes;
      the multitrack's notes regions place a box's lanes in the box's scope, so
      its playback's `notes` takes the id spaces -- the core ABI moved to 78.
      A curve's control is its target's `control`, else `bend`, `pressure` or
      `timbre`; a bare CC drives nothing on a synth. Both `edit_notes`
      examples' lane now drives `amp`, which the default def has. The user's
      ear tests found four things, fixed before closing: a slot's release is
      its `gate` port, which the note graph now always answers (the notes
      hung); a control a curve drives is not a port, since setting a mapped
      control unmaps it (the note's own `amp` took the level back from the
      lane); a note's readers are `ev.curve`, which holds while the transport
      is stopped -- the stop and the locate after it moved every curve a
      releasing note reads, a click and a jump in pitch -- and glides 10 ms,
      so a lane edited under the play line does not step; and a note's old
      table is given back when the next pass starts rather than on the next
      edit, whose buffer number could hand a sounding note another curve. A
      slot a lane forgets before it runs now goes with its graph's state, its
      buses and its voice's `ends`. A later ear test (a loop's wrap clicked in
      a releasing note) made the graph keep the two scopes as designed: a
      note's own curves are read in its own time (`ev.local`, counting from
      its start), so a wrap, a stop or a locate leaves its envelope where it
      was, and each channel curve it reads goes through a hold (`ev.hold`)
      that its `gate` closes -- the channel reaches a note from its on to its
      off, and after that the note keeps the last value.)*
    - ✅ **X3.11d - MIDI in and out.** Messages into curves when a `.mid` is
      read (a zone's member channels into per-note curves -- the half of
      `M35`'s acceptance moved here), and curves into messages when a
      sequence is written or played as MIDI (MPE for a per-note bend). With
      the editor's MIDI destination comes the rule deferred from X3.11b: the
      roll offers only the per-note curves that destination can say, and it
      **shows which MIDI it is editing** -- 1.0, 2.0 or MPE -- which helps the
      reading (the user, 2026-09-30).
      *(Designed with the user 2026-09-30, in four parts, one commit each and
      in this order. MIDI 2.0 is in, not deferred: the `midi2` crate is
      already a dependency of `clausters-midi`, which writes a notes-only
      SMF2CLIP, and the server normalizes to MIDI 2.0 resolution while
      reading MIDI 1.0 bytes. The roll already represents what 2.0 says per
      note.)*
      - ✅ **X3.11d1 - The sequence's MIDI spec.** `EventSequence.midi`, a
        `MidiSpec` -- the user's name for it, since it covers two protocols
        and a specification over them: `"1.0"`, MPE (its zone) or `"2.0"`, or
        none -- a sequence for the
        server, where every curve is legal. Set by reading a file, changed by
        a verb in both clients (`set_midi`/`setMidi`), kept in the document.
        A spec admits the curves it can say: per note, MIDI 1.0 has poly
        pressure alone, MPE bend, pressure and timbre, 2.0 those and per-note
        controllers; a lane in a MIDI spec is a channel message (CC, bend,
        channel pressure, timbre as CC 74), never a bare `control`. A curve
        the spec cannot say is refused, and so is a spec the curves already
        there cannot be said in. The roll shows the spec in the corner under
        its keyboard, beside the ruler. *(Shipped 2026-09-30: the document's
        `MidiSpec` and `CurveKind`, the `midi` intent, the refusals; a file
        read is MIDI 1.0; the roll's `midi` prop and its caption; `midi` /
        `set_midi` and `midi` / `setMidi` on both clients' sequence. Seen by
        eye with `X3.11d2`'s example, which reads a file.)*
      - ✅ **X3.11d2 - `.mid` in and out (1.0 and MPE).** Reading: a
        channel's stream of CC, bend (through the channel's RPN 0 range, 2 by
        default) or channel pressure becomes a lane of step points; poly
        pressure the expression of the note on its key; a declared MPE zone
        (its MCM) makes each member channel's bend (48 by default), pressure
        and CC 74 the expression of the note on it, and the master's streams
        lanes over the whole zone; the RPNs are consumed, and only the
        discrete gestures (program change, sysex, raw) stay events. Writing:
        lanes as channel messages, steps as they are and ramps sampled where
        the MIDI value changes; expression as poly pressure in 1.0, or on a
        member channel per note in MPE (the zone messages first); a curve
        with no spelling refused with why. Playing a sequence to a MIDI
        destination plays the same render. *(Shipped 2026-09-30: the
        document's `events::midi`, read and write, with a round trip through
        MPE in its tests. A curve with no spelling is refused when it is made,
        by `X3.11d1`'s spec, so a write never meets one; a sequence with no
        spec is written as MIDI 1.0 and what 1.0 cannot say is left out, as an
        OSC event is, and a 2.0 sequence is written as MPE until `X3.11d3`
        gives it a file of its own. Both clients' `midi_messages` /
        `midiMessages` is the render in beats, and a MIDI destination plays
        it. The example is `editors/edit_midi_file`, a pair; the page keeps
        its file in the origin private file system.)*
      - ✅ **X3.11d3 - SMF2CLIP in and out (2.0).** The clip file whole,
        read and written: lanes as 32-bit channel messages, expression as
        per-note pitch bend, 32-bit poly pressure and a per-note controller
        for timbre. *(Shipped 2026-09-30. The timbre is the **registered**
        per-note controller 74 -- Sound Controller 5, CC 74's meaning -- not
        an assignable one, as the standard has it; a per-note CC is an
        assignable per-note controller. `clausters-midi` writes a clip of any
        UMP packets and reads one back (`write_clip_ump`, `read_clip`, MIDI ABI
        5, in wasm too); the document encodes a sequence as Channel Voice 2
        and Flex Data (`to_ump` / `from_ump`, the tempo map as Set Tempo);
        both clients' `to_clip` / `from_clip` and `toClip` / `fromClip`. The
        example pair writes the chord again as a clip.)*
      - ✅ **X3.11d4 - MIDI 2.0 into the server** *(the user, 2026-09-30:
        MIDI 2.0 has to be an input for the server, by MIDI port or by a
        playback lane -- left for later if need be, but written down as
        pending, and better now if it can be done)*. A UMP parser for
        Channel Voice 2 in the actuation; a lane's `ump` list beside `midi`;
        per-note bend, pressure and controllers reaching the note's voice
        through the per-voice path MPE zones already use. The live transport
        (UDP MIDI 2.0, ALSA's UMP) is the open decision `PLAN.md` names; if
        it does not land here, this entry stays open.
        *(The lane and the network door shipped 2026-09-30: `crate::midi::ump`
        reads Channel Voice 2 into the actuation's `ChannelVoiceMessage`,
        whose resolution was already MIDI 2.0's, and the per-note messages
        into `PerNote`; `/midi_ump` plays packets now, a lane's `ump` list at
        their positions, read among its MIDI bytes; a per-note bend retunes
        the note's voice, a registered or assignable per-note controller sets
        the control its number is mapped to. **Closed there** -- the user,
        2026-09-30: the transport is for instructions internal to the server
        for now, and that is what has to work; the lane and `/midi_ump` are
        both, and neither depends on the operating system. The port a device
        would send packets through is external input, and it moved to root
        `PLAN.md`, Future directions, "MIDI 2.0 from outside the server".)*
    - ✅ **X3.11e - Between the scopes.** The two edits: a lane into its
      notes' expression, and the notes' expression into a lane, refused under
      overlap. *(Shipped 2026-09-30. The user set the weight between them:
      the main case is the lane into the notes, so that an event carries as
      part of its structure the lanes that made its expression; the way back
      is wanted but does not hold in every case -- and does hold for a chord
      whose lane was given to its notes, since they all have the same curve.
      So `lanetoexpression` gives each note on the lane's channel the stretch
      its span covers and the lane goes (a bend kept beside the copies would
      be heard twice); a note with its own curve over the control keeps it,
      except a bend, which adds and is refused; and `expressiontolane` is
      refused only where two notes that sound at once differ over the time
      they share, rather than under any overlap. The document's
      `events::scopes`; `lane_to_expression` / `expression_to_lane` and
      `laneToExpression` / `expressionToLane` on both clients; the
      `edit_notes` pair gives its level lane to the notes and gathers it
      back.)*
  - ✅ **X3.12 - The Hz domain.**
    *(Shipped 2026-09-28: the `notes` element reads `axes.y.unit` `"hz"` --
    its notes, their report and its compass in hertz, converted at the wire
    to the pitch its rows are, since a log frequency is a linear pitch; the
    axis ruled by the spectrogram's own `ruler::hz_ticks` in place of the
    keys, and a drag that snaps to nothing (`boxes::snap_row` with a step of
    0). The crate's roll sends the unit and a window in hertz fitted to the
    notes, and a named domain (`"midi"`, `"hz"`) opens it; both clients'
    `NotesEditor` and `edit` take `y_axis` / `yAxis`. A trajectory in the
    plane -- a glissando, a per-note bend -- is expression, `X3.11`.)*

  **Related:** the examples that edit notes through the raw event and have no
  history (`clients/python/PLAN.md`, "Half the editors a hand can use have no
  history") -- that entry closed on 2026-09-21 with `pianoroll` and
  `pianoroll_midi` as its only survivors, and **hands them here**, because
  porting them onto the editing seam waits on what this milestone edits; configurable key bindings (`clients/gui/PLAN.md`, `G36`) and the
  interaction-vocabulary entry beside it ("The whole interaction vocabulary is
  provisional…"); and an application inside
  another (`crates/clausters-document/PLAN.md`, Future directions), which is what
  would let a notes editor stand inside a multitrack or a script's window.
  And a defect of today's write-back, fixed in the host because the host's
  reading survives this milestone: an edit in the roll rewrote every note's
  amplitude ("A roll's edit sends every note at its velocity's amplitude",
  Found by use).

- ✅ **X4 - The points editor: the automation editor seen on its own.**
  *(Undecided 2026-09-14 -- the user was in doubt: it may be good for it to
  have undo, and perhaps chrome could be added to it. Decided and done
  2026-10-02, at the user's request: it is an application, so that it adopts
  the document and can be integrated with the other editors later; it is an
  automation editor seen on its own, which is also how an envelope is made; and
  the user asked that it duplicate no code the multitrack's and the roll's
  automation already have.)* What existed: the `bpf` element, `clausters_editing::points`
  (its view projection since `O25`), and `PointsDomain`/`PointsView`/
  `PointsEditor` in both clients, a client-side editor whose history was an
  external member.

  **What it is now.** `clausters_apps::points`: the editor holds the
  document's `Automation` -- the curve a track, a region, a sequence and a
  note hold -- shared (`Arc<Mutex<_>>`) by every points editor opened under
  one key, and edits it with the `points` vocabulary (`setpoints`), the edit a
  multitrack addresses to a curve it holds as `SetAutomation`. The window, the
  value axis and the time span it keeps while open (both only grow; a declared
  axis is the floor), the turn, the entry and its inverse, and the step
  (`Effect::Points`) are the crate's. It joins the context as
  `Member::Points` through the `openPoints` verb, so no symbol crossed either
  ABI. Both clients' `PointsEditor` is a handle that keeps what a language
  owns: the socket, handing the crate the curve as the script holds it
  (`sync`), and writing back the points each edit and each step leave
  (`PointsDomain.write`), since an `Env`, a `Bpf` and a client's `Automation`
  are objects of the client's.

  **`selection` went** in both clients, with its `_observe` / `observe`
  override. A curve has no transport, so the time range a sweep leaves is the
  editor's own `span`, `(start, end)` in the curve's seconds, settable and
  drawn as the band a sweep leaves; `selected` is the points inside it -- and
  inside its value band, for a sweep with height -- as the `(t, v, shape,
  curve)` quads `to_points` speaks.

  **One reading of a point on the wire.** The conversion between a document
  point and the `t v shape curve` numbers was written four times -- the curve
  projection, the multitrack's automation out and back, the roll's curves out
  and back. It is `clausters_editing::points::{quad, point, same}` now, and
  the three read through it.

  **Left open:** the range and the selection themselves -- no hand can
  leave them and no operation of this editor reads them, since `C61`'s rule
  was decided for the multitrack, the roll and the audio editor and read
  here as one for every editor (`clients/gui/PLAN.md`, Found by use, "The
  points editor carries a range and a selection it has no use for";
  *closed 2026-10-03*: both went, with the `selection` gesture, the `span`
  and `selected` verbs and the `sel_*` props). And opening an automation a
  multitrack or a sequence holds in this
  editor, sharing it rather than a copy, is the integration this milestone
  prepares and does not do; it goes with "An application inside another"
  (`crates/clausters-document/PLAN.md`, Future directions).

- ✅ **X10 - The points editor's ranges, as rules.** *(Asked for by the user
  2026-10-02, after `X4`: a points editor can be given rules with ranges --
  a normalized `Env` is time on x and amplitude on y, both from 0 to 1, and
  another curve wants another range, as an automation does; and the rules are
  the application's, integrated and visible by default; and a value readout
  like the roll's, the points' values against the rules and the shape of the
  curve being edited. Done the same day.)*

  **What exists.** Two rules for a curve's value range, written apart: the
  roll's (`clausters_editing::notes`, by what the curve automates -- a CC 0 to
  127, a bend 2 semitones either way, pressure, timbre and a control 0 to 1 --
  with a `min`/`max` on the target winning) and the multitrack's
  (`clausters_editing::multitrack`, the target's `min`/`max` over 0 to 1).
  The points editor has none: its declared `min`/`max` are a floor the axis
  grows past, so a hand drags a point anywhere, and it draws no rulers.

  **The design** *(recommended, the user agreeing)*:

  - **One rule for a parameter's range**, `clausters_editing::points::range`
    over what a curve automates, read by the roll, the multitrack and the
    points editor alike.
  - **A curve's bounds**: its values from the parameter it automates (an
    `Automation`'s target), or from what the caller declares (`min`/`max`),
    which wins; its time from what the caller declares (`start`/`end`). A
    curve that says nothing about a range (an `Env`, a `Bpf` with no
    declaration) keeps today's derived axis, which grows.
  - **A bound is a rule, not a floor**: the axis is the bound and holds, the
    host keeps the hand inside the field it draws, and the crate clamps
    every point a gesture reports into it.
  - **Visible by default**: the window draws the curve with its time ruler
    (in the curve's own seconds) and its value ruler on.
  - **A readout**, the roll's: a `curve` standing on its own reads, in its
    field's corner, the point under the pointer (its number, time and value
    against the range) or the curve's value there, and the shape of the
    segment (`clausters_core::envshape::shape_name`).

  **What shipped.** `clausters_editing::points::range`, which the roll's
  curves and the multitrack's now read (a CC automation on a track is drawn
  over 0 to 127, where the multitrack's own copy drew it over 0 to 1);
  `clausters_apps::points::Rules` (`Rules::of` the curve and the declaration,
  `keep` on every gesture), the window's rulers and the `rules` verb; the
  `target` an `Automation` carries handed to the crate by both clients, and
  `start`/`end` beside `min`/`max` on `PointsEditor` and `edit`, with
  `editor.rules` to read them; the host's `curve` readout. `edit_env` and
  `edit_curve` declare their ranges as rules, in both clients.

  **And then, by use** *(the user, the same day, over `edit_env`)*: a
  segment's shape is set as an `Env`'s is, by selecting the segment and
  choosing it from the menu that moves to a column beside the curve; the
  readout follows the hover and the edit, and sits top right; the
  selected segment is heavier and in another color, since the lit one was the
  trace's own green; the hint label the example appended goes, since an
  application carries none; and the script's controls stand in that column,
  wide enough to read as controls. Done: the host's `curve` selects a segment
  on a press (`"segment"` event, `segment` prop) and draws it in the
  selection's color, the lit one in the accent's quiet form; the application's
  window is the curve over a row of controls holding the shape menu over
  `envshape::shape_name`'s names, into which both clients append `extra` --
  first a column beside the curve, then, by the user, the row under it, the
  controls side by side in a quarter of the width (how a window's controls
  are laid out is widget chrome, a later milestone's); a choice there is an
  edit through the same path a gesture takes. The curve's readout redraws on
  hover -- a window asked for a frame per pointer move only for timelines and
  signals, never for an element that said it draws a readout -- and reads
  while a point or a segment is being edited too. The web `EditOptions` gained the `extra` the Python `edit` already
  took and the example already passed.

- ⬜ **X5 - The score editor.** The third of the three applications over the
  document (`crates/clausters-document/PLAN.md`, `O24`). *(Planned 2026-10-05
  with the user, after a by-eye pass over `notation/score_editor`; the user's
  observations from that pass are the requirements below.)* ***Built
  2026-10-05, every step `X5.0` to `X5.9`, and not closed**: it closes with
  the user's eye and ear pass over `notation/score_editor` (the user: "cuando
  termines reviso los resultados y debugueamos"). The first looks of that
  pass are under "Found by use" -- the page that was in no window, the
  cursor, the written breaks, the symbols, the page's head, the dialogs, a
  text typed over where it is drawn, all fixed there; note entry, built as a
  mode with an edit cursor, which is also how the second voice is written; a
  written page break, laid out in runs of pages; the ○ palette entries, each
  the model grown with its sound; a channel group edited as one; a written
  pitch that moves with the sounding one; New and Close in the File menu;
  the standalone host's playback and export. What stays open: the sound,
  which nobody has listened to.* The notation model
  and what is still open about editing a page are the N track's
  (`clients/gui/PLAN.md`: `N7` what opening a foreign score preserves, `N8`
  which element admits which edit, `N9` a score as a box of the multitrack);
  `N8` is taken here, as `X5.0`.

  **What is under it.** The model is `clausters_core::notation`: a `Sheet`
  (flat content over a metric `Grid`), its operations (`Op`, listed by
  `catalog()`), the MEI emission and the reader, the interpreter (`perform`)
  and `Score`, the engraver-driven document with its one undo stack. **An edit
  is a model operation and never the engraver's editor**: verovio lays the
  page out, the model edits it (`Score::apply` says why -- one meaning of an
  edit, reachable from a process with no engraver in it). The host's `score`
  element draws a display list, picks an element by its MEI id, drags a pitch
  and reports a place for note entry. A `Score` joins the editing context as an
  external member. The four Python examples in `clients/python/examples/
  notation/` each assemble an editor in the script -- `score_editor.py` the
  fullest: buttons per verb, a selection, an undo -- which is what this
  milestone moves into the crate, as `X3` moved the roll's.

  **The requirements** (the user, 2026-10-05, paraphrased):

  1. The edit vocabulary is organized in **palettes**, one per kind of element.
  2. Only what has a pitch drags with **ledger lines**. Today a slur, a time
     signature, a hairpin, a rest, even a measure's staff, drag and grow ledger
     lines; accidentals need checking too.
  3. Each kind of element has **its own editing behaviour**.
  4. Two **views**, chosen alternately: a **page** view, whose size is
     configurable and fixed until changed and comes from real paper sizes, and
     a **continuous** view, one system scrolling left to right with no page.
  5. In the page view, **title, subtitle, lyricist and composer** have a
     layout of text fields, and so do **footnotes**.
  6. A **menu bar** with every action the application has -- the
     transformations (transpose, retrograde, inversion and the rest) among
     them.
  7. A **toolbar with icons** for the common input values -- notes, durations,
     ties, accidentals, common articulations, tuplets, the voice (1 or 2 per
     staff) -- and the **transport** (play/pause, rewind, loop).
  8. A slur is made by choosing its **first and last notes**, not a count.
  9. **Several elements selected at once** with ctrl+click, and an edit applied
     over the range.
  10. A drag that starts on **no staff and no element pans** the view, and an
      element's **hit area follows its shape** (a spatial partition where it
      pays).
  11. The score **renders into an `EventSequence`**, so it plays on the
      server's transport. The rendering is organized by **voices and channels**
      (MPE or MIDI 2.0 where a note needs its own curve): a dynamic can belong
      to a voice, so can a crescendo or a glissando, and what is rendered
      divides into actions on a note, on a channel and on a group of channels.

  **What verovio engraves, by palette.** verovio 6.3.0's own editor
  (`editortoolkit_shared.cpp`, `editortoolkit_cmn.cpp`) edits little and is not
  the edit path: `drag` moves only an element with a pitch, `keyDown` steps a
  pitch or a value, `insertNote`/`insertRest`/`insertMeasure` and its cursor
  write notes, `insertControl` attaches any control event to a start and an end
  id, `set` changes any attribute and `delete` removes. What it does define is
  the **set of elements it engraves** (its `ClassId` families: layer, control
  and system elements), and that set is where the palettes come from. *(The
  user, 2026-10-05: the model is ours to make, and its elements are the ones
  verovio has.)* So the division is fixed: **the elements are verovio's** --
  what a palette offers is an element verovio engraves, under its MEI name --
  and **the model and its verbs are ours**. ● the model holds it today; ○ the
  palette waits on the model growing it -- an item or a field, its emission and
  reading, and an `Op`. *(Every ○ entry was grown 2026-10-05, "The ○ entries
  are notation the model does not hold" under "Found by use"; each now names
  where the model holds it.)*

  - **Notes and rests** (the toolbar's input)
    - ● `note` -- one pitch with a written value, the element entry writes.
    - ● `chord` -- several pitches sharing one stem and one value.
    - ● `rest` -- a silence of a written value.
    - ● `mRest` (read as a rest) -- a rest filling a whole measure, centred in
      it whatever the meter.
    - ● `multiRest` (`Grid::multirests`, `SetMultirests`) -- several empty
      measures drawn as one numbered bar, the usual sight in a part.
    - ● `space` (read as a rest) -- time a voice holds without drawing
      anything.
    - ● `dots` -- augmentation dots, part of the value (the item's ratio).
    - ● grace notes (`grace`: `acc`, `unacc`) -- an ornamental note that takes
      no time from the bar.
    - ● `tuplet` (as exact ratios) -- a group played in the time of another
      count, three in the time of two.
    - ● `beam` (read; the engraver's unless written, `N7`) -- notes joined
      under one beam.
    - ● `bTrem` / `fTrem` (`Marks::tremolo`; the `ftrem` spanner) -- a tremolo
      on one note, or alternating between two.
  - **Accidentals and pitch**
    - ● `accid` (`alter`, and `forced` for a courtesy sign) -- a sharp, flat,
      natural or double, written or implied by the key.
    - Transposition, the octave and enharmonic respelling are verbs over notes,
      not elements (`Transpose`, `MoveSteps`, `SetPitches`).
  - **Articulations** (`artic`)
    - ● the marks of attack and release by their MEI names -- staccato,
      staccatissimo, accent, tenuto, marcato, stress, spiccato -- and the
      technique marks MEI names beside them (up-bow, down-bow, harmonic, snap,
      open, stopped), each a sign on one note.
  - **Ornaments**
    - ● `trill` -- a rapid alternation with the note above, with a wavy line
      when it lasts.
    - ● `mordent` -- one quick alternation with the note above or below.
    - ● `turn` -- the four-note figure around the main note.
    - ● `fermata` (an ornament in the model, a control element in MEI) -- a hold
      of no fixed length over a note, a rest or a barline.
    - ● `ornam` (`Marks::ornament`, any name past the four) -- any other
      ornament, named by its glyph.
    - ● `arpeg` (`Marks::arpeggio`) -- a chord rolled from its lowest note up,
      or down.
    - ● `gliss` (the `gliss` spanner) -- a slide drawn as a line from one note
      to the next.
    - ● `breath` / `caesura` (`Marks::breath`) -- a breath, and a full stop of
      the line.
  - **Lines between two notes** (spanners, made from a selection's first and
    last notes)
    - ● `slur` -- a curve over a phrase, from a first note to a last.
    - ● `tie` (the item's `tie`) -- two notes of one pitch joined into one sound.
    - ● `lv` (`Marks::ring`) -- a tie into nothing: let it ring.
    - ● `hairpin` (`crescendo`, `diminuendo`) -- a wedge for a gradual change of
      loudness.
    - ● `phrase` (a spanner, as are the four below) -- a phrase mark distinct
      from a slur, for analysis.
    - ● `octave` (`8va`, `8vb`, `15ma`, `15mb`) -- a line moving the written
      notes by octaves.
    - ● `pedal` -- the sustain pedal pressed and released, as signs or a
      bracket.
    - ● `bracketSpan` (`bracket`) -- a bracket over a stretch of notes.
    - ● `beamSpan` (`beamspan`) -- a beam across a barline or across staves.
  - **Dynamics and text over the music**
    - ● `dynam` (one per note today) -- a level, `pp` to `ff`, `sf`, `fp`,
      under the staff.
    - ● `tempo` (a `Control`, as are the two below) -- a tempo mark, as words,
      as a metronome value or both.
    - ● `dir` -- a free direction ("dolce", "pizz.") at a point in time.
    - ● `reh` -- a rehearsal mark, a letter or number in a box.
    - ● `fing` (`Marks::fingering`) -- a fingering number over a note.
    - ● `harm` (`Marks::harmony`) -- a chord symbol or a figured bass over the
      staff.
    - ● `syl` / `verse` (`Marks::lyrics`) -- lyrics, a syllable under a note,
      verse by verse.
  - **Measures and structure**
    - ● `measure` (the grid) -- the bar: inserted, removed, selected as a range.
    - ● `meterSig` (`SetMeter`) -- the time signature, at the start or as a
      change.
    - ● `keySig` (the sheet's key, and `Grid::keys`, `SetKey`) -- the key, and
      a change of it inside the score.
    - ● `clef` (`Staff::clef`, and `Staff::clefs`, `SetClef`) -- a staff's
      clef, and a change of it inside the staff.
    - ● `barLine` (`SetBarline`) -- how a measure ends: single, double, final,
      repeat start or end, dashed, invisible.
    - ● `ending` (`Grid::endings`, `SetEnding`) -- first and second endings
      (voltas) over measures.
    - ● `repeatMark` (`Grid::marks`, `SetMark`) -- segno, coda, da capo and dal
      segno.
    - ● `mRpt` / `beatRpt` (`Grid::repeats`, `SetRepeat`; `Marks::beat_repeat`)
      -- repeat the previous measure, or beat.
    - ● `sb` / `pb` (`SetBreak`) -- a system or page break the writer asks
      for.
  - **Staves**
    - ● a staff's line count, its label (the instrument's name, and its short
      form) and a transposing staff (`Staff::lines`, `label`, `abbr`,
      `transpose`, `SetStaff`). What a staff of other than five lines means
      for pitch is still open ("A selected staff is edited by its line
      count", `clients/gui/PLAN.md`, Future directions).
    - ● `staffGrp` (`Sheet::groups`, `SetGroups`) -- a brace or bracket
      grouping staves (a grand staff, a section).
    - ● voices (`ToVoice`) -- a second line on one staff.
  - **The page's text** (`pgHead`, `pgFoot`)
    - ● title, subtitle, composer, lyricist, arranger, translator, copyright
      and footnotes (`Header`, `SetHeader`).

  **The transformations are the menu's too.** Every operator in `catalog()`
  -- `transpose`, `invert`, `retrograde`, `repeat`, `stretch`, `concat`,
  `stack`, `insertMeasures`, `removeMeasures`, `setMeter` -- is an entry of a
  **Transform** menu, acting on the selection (its `Span`: the measures it
  covers, or all) and asking for its parameter where it has one (an interval, a
  factor, a count) in a small dialog. Each is one undo step, as every `Op` is.

  **Each kind of element, and what a hand does to it** (`N8`'s answer, for
  this application):

  | Element | Press | Drag | Ledger lines | Keys |
  | --- | --- | --- | --- | --- |
  | note, chord | selects | vertical: diatonic pitch | yes | up/down a step, ctrl an octave |
  | accidental | selects its note | its note's pitch drag | its notehead's | its note's |
  | rest | selects | none | no | -- |
  | slur, tie, hairpin, other spanner | selects | none: its ends are notes, re-chosen from a selection | no | -- |
  | dynamic, articulation, ornament | selects the mark on its note | none | no | delete removes the mark |
  | clef, key, meter | selects | none | no | a palette entry or the dialog sets its value |
  | barline | selects | none | no | a palette entry sets its kind |
  | staff lines | selects the staff | none | no | -- |
  | page text | selects | none | no | a double click edits it |
  | blank paper | clears the selection | pans the view | -- | -- |

  Where a page takes note entry, a press on a staff's blank space writes a
  note (as today); a press off every staff pans. Where an element sits on the
  page is the engraver's, so no element is placed by dragging it -- the same
  line the N track draws when it refuses to store a layout nobody chose.
  **The rule lives in one place**: the display list carries each id's
  **kind** (the SVG class the walk already reads, `is_element_class`) and
  `clausters_core::notation` holds the table of what a kind admits, which the
  host reads -- never a list per client.

  **Selection.** A click selects one element; ctrl+click adds or removes one;
  shift+click extends a contiguous range in time, across the staves it spans
  (the field's convention, beside the user's ctrl+click); a click on a
  measure's empty space, where entry is off, selects the measure. The host's
  `selected` becomes a list. Every palette and menu verb applies to the whole
  selection, and a spanner is made from its first and last notes in time.

  **The hit area.** Today each primitive is one box (an ellipse for a
  notehead), searched linearly, with the smallest sounding element first. It
  becomes the **shape that is drawn** (`E22`'s rule): the point is tested
  against the element's own tessellated triangles, which `tess.rs` already
  builds, with a hit slop for hairlines (a slur, a hairpin, a stem) measured as
  a distance to the stroke. In front of that, an index over the boxes, built
  once per display list since a page does not move between engravings. The
  user named binary space partitioning; a page is static and every query is a
  point, which a bounding-volume hierarchy or a grid per system also answers,
  so the structure is chosen by measuring hover over a full page.

  **The two views.** A `one of several` entry in the View menu and a segmented
  control on the toolbar switch them; the selection and the measure in view
  survive the switch.

  - **Page.** The paper is fixed until it is changed: a window resize shows
    more or less of the page and never re-flows it, and zoom is the view's
    transform rather than a new engraving. Every page is engraved and laid out
    in the score's `plane`, one after another. The engraver's options carry it
    (`pageWidth`, `pageHeight`, the four `pageMargin*` and `landscape`, all in
    tenths of a millimetre; its default, 2100 by 2970, is A4), and the staff
    size is its `unit`, half a staff space (a staff is eight units high, so 9
    gives a 7.2 mm staff). The paper offered, from the sizes printing and the
    orchestral libraries' preparation guidelines use: A4 (210 by 297 mm), A3
    (297 by 420), B4 (250 by 353), Letter (8.5 by 11 in), Tabloid (11 by 17
    in), 9 by 12 in (the floor those guidelines set for a part), 10 by 13 in
    (the part size they recommend), octavo (6.75 by 10.5 in, choral music), and
    a custom size; portrait or landscape. The guidelines put a part's staff at
    7 to 8.5 mm and a score on 11 by 17 in or B4. Which `breaks` mode honours
    the writer's breaks and fills in the rest (`auto`, `smart`, `encoded`) is
    settled against the engraver.
  - **Continuous.** `breaks: none`: one system the length of the music, no
    header or footer, the page fitted to the content; the plane scrolls
    horizontally and the playing cursor scrolls it.
  - **Page setup is the document's**: the writer chose the paper, so it is a
    field of the `Sheet`, written into MEI as the `scoreDef`'s `page.width`,
    `page.height`, `page.topmar`, `page.botmar`, `page.leftmar`,
    `page.rightmar` and `vu.height` (the staff size), read back by the reader,
    and turned into engraver options by `engrave_options`, one rule for every
    client. The view (page or continuous, the zoom) is the window's, not the
    document's.

  **The page's text.** The engraver's running elements are `pgHead` and
  `pgFoot`, each a set of text blocks placed by `halign` (left, centre, right)
  and `valign` (top, middle, bottom), on the first page (`func="first"`) or
  on every page (`all`). Its generated header follows the field's convention:
  the title centred and large, the subtitle smaller under it, lyricist and
  translator on the left, composer and arranger on the right, the page number
  from the second page on. The model's `Header` grows into that layout: each
  field (title, subtitle, composer, arranger, lyricist, translator, copyright,
  footnote lines) in a cell of the head's or the foot's three by three grid,
  first page or all, with the convention as the default and copyright at the
  foot of the first page. It is emitted as encoded `pgHead`/`pgFoot` (the
  engraver's `header` and `footer` set to `encoded`), so what is placed is what
  is drawn. A double click on a text block edits it in place; a Page text
  dialog (`G40`'s `modal`) holds a field per cell.

  **The window** (`G37`-`G40`'s elements):

  ```
  menu bar
  toolbar: [entry] [whole ... 64th] [dot] [rest] | [tie] [bb b nat # x] |
           [stacc acc ten marc] | [tuplet] | [voice 1|2] | [page|continuous]
           -- spring -- [rewind] [play/pause] [loop]
  [palettes: titled, collapsible groups] | split | [plane: the score, bars]
  status: the selection . the input value . bar:beat . the view
  ```

  - **Menus.** *File*: new, open (what the reader takes: MEI, MusicXML, ABC),
    save (MEI), save as, export (MIDI and a clip through the sequence, below),
    page setup, close. *Edit*: undo, redo, delete, silence, select all, select
    measure, select staff, and ○ cut/copy/paste, which have no `Op` yet.
    *View*: page or continuous, zoom in, out, to the width, to the page,
    show palettes, show toolbar. *Notes*: entry, the values, dot, rest, tie,
    the accidentals, a step or an octave up and down, respell, voice 1 or 2.
    *Notation*: slur, crescendo, diminuendo, and a submenu each for dynamics,
    articulations, ornaments, tuplets and grace notes. *Measures*: insert
    before or after, remove, meter, key, clef, barline, system break, page
    break. *Transform*: the operators above. *Text*: the page text dialog.
    *Play*: play/pause, rewind, loop, play from the selection. The keys beside
    the entries are `G36`'s table.
  - **The toolbar's state is the application's**, like the notes editor's
    cursor: the value, the dot, rest mode, the accidental, the voice and the
    tie for the next note entered, shown by the toolbar through its props. An
    articulation or an accidental pressed with a selection applies to it;
    pressed with none, to the next note written.
  - **Icons.** `G38.3`'s symbol set is seventeen shapes; note values, rests,
    accidentals and articulations are **SMuFL** glyphs, and verovio already
    has them as vector graphics: one SVG path per codepoint in its resources
    (`data/<font>/<codepoint>.xml`, read through `default_resource_path`),
    which is where a display list's glyph table comes from too. So an icon of
    this window is a SMuFL codepoint whose outline the **application** sends
    with the window, from the engraver's own resources, and the host draws it
    the way the `score` element draws a glyph. The host bundles no music font,
    and a toolbar icon is the same shape as the sign it writes on the page.
  - **A context menu** on an element is `G37.2`'s open question, a context menu
    per part of a heavy view; until it is answered the score answers with one.

  **The `Sheet` is the reference** *(decided with the user, 2026-10-05)*.
  The score's structure is the `Sheet` -- exact rational values, flat voices
  over a grid of its own, spanners over item ids, the header -- and every
  operation, the reading and the emission work on it. Events are not the
  score's representation but its render: an `EventSequence` holding a score
  would need rational time, the grid, spanners and the page as well, and every
  operation written again over events. The notation keys (`X5.7`) are what the
  events say about the page they came from, not a second model of it.

  **The score into an `EventSequence`, one way.** *(The user, 2026-10-05:
  `edit` returns what every editor returns, its handle, and not the data; the
  notation shares the `EventSequence`, so a score is turned into a roll with
  its automation; and that conversion goes one way only -- from the roll back
  to a score takes decisions, and is implemented later.)*

  - **What `edit` opens and returns.** `edit(score)` opens the score editor
    over a symbolic score, the client's `Score`, and returns the editor, as
    for every structure ("One `edit` for every application", at the top of
    this file). The edited data is read on the score passed in, and the
    editor adds nothing to the contract.
  - **The conversions are the score's own methods, not `edit`'s** *(the
    user, 2026-10-05: `edit` returns the handle to the score, and what is made
    from it afterwards is made by other functions or methods over that
    representation)*. `score.render_events()` renders it into an
    `EventSequence` -- the verb a `Timeline` and a pattern already have, with
    the same one-way meaning -- each event keeping its item's id. Opened as a
    roll (`edit(score.render_events())`), the score is notes with their
    automation, and what the roll does to it stays in that sequence: nothing
    travels back to the score. The editor's own playback renders the same way,
    inside the crate.
  - **The score is saved as itself** *(the user, 2026-10-05: the edited score
    can be saved as such)*. Its document is MEI, and what is saved is the
    edited score whole -- notes, marks, spanners, the grid, the page setup and
    the page's text -- not a render of it: reopened, it is the same page, with
    no reading to decide. From the File menu (save, save as) and by a method
    of the score in both clients, writing and reading a file; today the client
    `Score` gives its MEI as text (`mei()`) and is built from text, so the file
    verb is new, and its name follows the file verbs the other structures
    already have, settled with the code. In a page the path is the origin
    private file system's.
  - **The way back** -- a sequence read into a `Sheet`: onsets snapped to
    written values, pitches spelled, voices found, curves read as dynamics and
    hairpins, and what a sequence edited in the roll means to the score that
    rendered it -- is Future directions, "From the roll to the score". So is
    opening a sequence of plain numbers in the score editor, which is that
    reading.
  - **As a client's data.** What the editor made is in the events, so turning
    it into a client's primitive data is a later step, outside this milestone
    (Future directions, "A sequence as primitive data, by the keys chosen").
  - **Where it lives.** The conversion goes to Rust, beside the sequence's
    others (`clausters-document`, `events`, over the core's `notation`). The
    Python client's `sheet_from_timeline` and `to_timeline`
    (`clausters/gui/notation/mei.py`) are a conversion written in one client;
    `to_timeline` is replaced by this one, not ported, and
    `sheet_from_timeline` waits for the way back.

  **The notation keys, without ambiguity.** The core reserves eight
  (`clausters_core::event::render::RESERVED`): `articulations`, `dynamic`,
  `ornament`, `grace`, `stem`, `spelling`, `accidental`, `tie`, documented in
  the Python book's "Seeing a timeline as a score". That was the start; for an
  event to carry a score without loss, each key has to name one fact, in one
  unit, with one owner. What is ambiguous or missing today:

  - **The written pitch.** `spelling` (`sharp`/`flat`) is a preference, not a
    spelling: it cannot write an F flat, an E sharp or a double sharp. And
    `alter` already means two things -- the pitch family's alteration of a
    `degree`, and the model's written alteration of a step. The written pitch
    needs a key of its own (a step, an alteration and an octave as written),
    and `spelling` stays the preference used only where there is none.
  - **The written value against time.** `dur` is beats to the next event and
    `sustain` the held length; the written value is a rational in whole notes
    (a triplet eighth is exactly `1/12`). A float in beats does not say a
    tuplet exactly, so the written value, or the tuplet, needs a key.
  - **Where it is written.** `staff` and `voice` are what the interpreter
    names on every note and are not reserved -- so today they would reach a
    synth as controls. They become notation keys, and with them the channel a
    voice renders on (below).
  - **A level written and a level heard.** `dynamic` is the mark; `amp`,
    `velocity` and `db` are the level. When both are on an event, the mark is
    the source and the level is its performance, re-derived after an edit.
  - **A grace note.** It takes no time from the bar: what its `at` and `dur`
    mean has to be stated.
  - **What has two ends** -- a slur, a hairpin, a glissando -- cannot be one
    event's key alone: the start names the end by event id, or the sequence
    keeps a list of spanners over event ids, as the `Sheet` does.
  - **What is not any note's** -- the meter and its changes, barlines, the
    key, the clefs, breaks, the page setup and the page's text -- is the
    sequence's own: a notation section of the `EventSequence`, beside its
    `tempo_map`, kept by the sequence so a round trip loses nothing.

  The definition is one table, in the core, read by both clients and by the
  books' reference -- each key, its type, its unit, and which way it is read.

  **What sounds, and the curves.** The interpreter's `Note`s (`perform`: the
  written and the held length, the level, the `staff` and the `voice`) give
  each event its playing keys. **Today `perform` yields no curves**: a dynamic
  and a hairpin are folded into each note's attack amplitude, read at its
  onset, so a crescendo over a held note does not move inside it and a roll
  would see velocities and no automation. It grows to yield curves in the
  scopes below, the voices get their channels, and a glissando waits on the
  model holding one (its palette entry). The sequence plays on the server's
  transport as any sequence does (`PLAN.md` `T8`, `T9`) and is what an export
  writes as `.mid` or as a clip. What a mark becomes is the interpretation's
  (data, replaceable), and it is organized in **three scopes**:

  - **The note**: its pitch, its two lengths, its attack level (a dynamic and
    an accent), and its own curves -- a glissando as its pitch bend, a
    crescendo inside one held note as its pressure. In MPE or MIDI 2.0 these are the note's expression
    (`X3.11`); in MIDI 1.0 a curve needs the note alone on its channel.
  - **The voice, as a channel**: each voice is a channel, so what governs a
    line is a lane of that channel -- the dynamic that prevails after it, a
    hairpin as a ramp (an expression controller, by the interpretation's
    choice), a glissando of a monophonic line as the channel's bend.
  - **A group of channels**: what governs several voices at once -- a staff's
    dynamic under two voices, a pedal over both staves of a piano -- is the
    same lane on each channel of the group, made and edited as one.

  The sequence's `MidiSpec` follows from what the score needs: MIDI 1.0 when
  no note carries its own curve, MPE or 2.0 when one does (a glissando inside
  a chord); none for a sequence played only by the server, where every curve
  is legal. Which controller a dynamic or a hairpin moves, and whether a level
  is the attack, a lane or both, is the interpretation's and is shown in the
  book. A channel group is new to the sequence -- today a lane belongs to one
  channel -- so how a group is named and kept is the render's first decision.

  **The steps**, each closing with its commit, in this order:

  - ✅ **X5.0 - Which element admits which edit** (`clients/gui/PLAN.md`,
    `N8`, taken here). The display list carries each id's kind; the table of
    what a kind admits is in the core; a drag moves only what has a pitch, and
    ledger lines follow only a notehead; an accidental's press is its note's.
    The hit test on the drawn shape and its index. Host and core only, tested
    without the application. *(Done 2026-10-05. `DisplayList::kinds` is the
    engraver's class per drawn id, read off the SVG; `notation::admits` is the
    table, and today only a `note` admits a pitch drag. An accidental needed
    nothing: the walk already gives a note's parts the note's id. The host's
    hit index moved to its own module (`score/hit.rs`): each entry is tested
    as the shape it is drawn as -- a glyph's or a fill's outline, a stroke's
    distance from its line, a notehead's oval -- first exactly, then within
    the hit slop, the staff's own lines taking none; a uniform grid of two
    staff spaces narrows a press to its neighbourhood, chosen over a binary
    space partition because a page is static and every query a point. Found
    on the way: both clients sent the page through a fixed list of keys that
    had never included `systems`, so the grand-staff fix never reached the
    host; each client now has one list (`SCORE_PAGE`) read everywhere, with
    `systems` and `kinds` on it.)*
  - ✅ **X5.1 - The application in the crate.** `clausters-apps::score`: the
    window, its props and the conversation (a gesture read, an `Op` applied, a
    page sent back as the `Outcome`), the `Score` a member of the editing
    context that the crate applies. The engraver is a port the caller hands
    in (the core's `Engraver`): the standalone host links `clausters-notation`,
    a page binds verovio in wasm, and the crate links neither. The doors over
    the C ABI and wasm, declared in `docs/bindings.md`; both clients' editor a
    handle over the crate; the standalone host opens a score file. The
    script-side editor of `notation/score_editor` goes, and the example opens
    the application. *(Done 2026-10-05. The engraver port is
    `notation::AnyEngraver` -- any engraver behind a box -- so the score both
    bindings hand out and the one the app holds are one type,
    `Score<AnyEngraver>`; the C ABI's score handle and the wasm `Score` now
    wrap it shared (`clausters_apps::score::Shared`), and
    `clausters_apps_editing_open_score` / `JsEditing.open_score` open the
    editor over it. The crate's part is behind its `notation` feature:
    `score::window` (the page in a scroll over a status line),
    `score::editor::ScoreEditor` (the three page gestures, a selection, the
    value a note is entered with) and `score::verbs` (move, scale,
    articulation, dynamic, ornament, clear marks, tie, silence, delete, voice,
    spanner from the first selected to the last, and any `Op`), each verb one
    entry through the context's new `act`. Opening writes the page from the
    model when the document's ids are not the model's -- a score read from a
    foreign file had ids no item answered to, so the first press selected
    nothing. Both clients' `ScoreEditor` binds the verbs as methods (`move`,
    `scale`, ... `operate`, the escape hatch, since `apply` is every editor's
    door for the host's messages), `selected`/`select` and `value`, and
    `edit(score)` opens it. The standalone host opens one with `--score`
    under a `score` feature that links the engraver, and the package's host is
    built with it. The example opens the application, its buttons calling the
    verbs; the playback stays the script's until `X5.8`, and the slur button
    selects two notes itself until `X5.2` lets a hand do it.)*
  - ✅ **X5.2 - Selection.** ctrl+click, shift+click, the measure; `selected`
    as a list; every verb over the selection; spanners from the first and last
    notes; the Transform menu over its span. *(Done 2026-10-05. The host
    keeps a list: a plain press replaces it, Ctrl toggles the element in it,
    Shift adds it, and a modified press reports its `mode` beside the id and
    starts no drag; `selected` takes one id or a list and every one is
    highlighted. The editor answers each press with the whole selection, since
    only the model knows time: a Shift reaches from the first selected item to
    the one pressed, across the staves between (`verbs::range`), and an item
    is selected as every element the page draws it as. The emitter now names
    each measure and each staff of it (`m3`, `m3s1`, read back by the core's
    `measure_id`), so with `entry` off a press on a staff selects that measure
    (`verbs::measure_items`). `transform` hands `transpose`, `invert`,
    `retrograde`, `stretch` or `repeat` the measures the selection covers, or
    everything with nothing selected; it is a verb of both clients' editor
    here, and the menu that offers it is `X5.5`'s. Both clients gained `entry`
    and `transform`, and the example's slur is made from a selection of its
    two ends.)*
  - ✅ **X5.3 - The views and the paper.** Page and continuous; page setup in
    the model and in MEI; the paper sizes, the orientation, the margins and the
    staff size; every page in the plane; pan from blank paper, zoom as the
    view's transform. *(Done 2026-10-05. `notation::layout` holds the papers,
    the `PageSetup` -- a field of the sheet, set by the `set_page` operation,
    written into the score definition as `page.*` and `vu.height` and read
    back -- the `View`, and the one function that turns the two into the
    engraver's options. Measured against the engraver: `auto` honours a
    written break and fills in the rest, so it is the page view's mode; and
    its `scale` is a zoom of the layout, so paper is laid out at 100 or a page
    holds more music than its staff size says. The engraver port gained
    `set_options` and `page_count`, and `Score::relayout` lays a document out
    again with its history intact. A page view is every page one under another
    in one display list (`DisplayList::stacked`), each in a frame drawn as
    fills so the paper's edge is never read as a staff; the drawing is sized
    by the paper's width across the window in both views, so a staff is the
    same size in either. The window's scroll pans both ways with bars, a drag
    on blank paper pans because the page declines that press, and the host
    gained `zoom: "ctrl"` on a plane -- the wheel scrolls and Ctrl with it
    zooms -- since a plane could only do one or the other. Both clients gained
    `layout` (the window's: it is not called `view`, which is every editor's
    picture), `page` and `set_page`, and the `set_page` sheet builder. A
    second window over one score shows the layout of the last one laid out:
    the engraver is one, and that is left as it is.)*
  - ✅ **X5.4 - The page's text.** The head and foot layout in the model,
    emitted as encoded running elements. *(Done 2026-10-05. The header gained
    `arranger`, `translator`, `copyright`, the footnotes and `places`, the
    cell a field was moved to; `notation::pagetext` holds the convention
    (`default_place`), writes the fields as `pgHead`/`pgFoot` blocks placed by
    `halign` and `valign`, each under an id that names it (`t-title`,
    `t-note-2`), and reads them back. Measured against the engraver: two
    blocks in one cell are written one under the other, an encoded head takes
    over the engraver's own, so the page number is written here from the
    second page on, and with no foot written the engraver draws none. The
    walk now names a block of page text by its own id -- it carried the head's
    -- and reads a run of white space as one space, which a page number
    written as a dash, a number and a dash had shown as three words and a
    column of blanks. The editor gained the `text` verb and says a pressed
    text's field and place on the status line; both clients gained
    `set_text` and the header builder its new fields. **Editing a text in
    place and the page text dialog moved to `X5.5`**: both need the window to
    grow widgets while it is open, which is the same thing the Transform
    menu's parameter dialog needs, so the three are built there on one
    mechanism.)*
  - ✅ **X5.5 - The menu bar and the toolbar.** Every entry above; the input
    state; the icons; the transport controls. With it, from `X5.4`: a dialog
    the window opens while it is up -- the page text's fields, a
    transformation's parameter, the page setup. *(A text of the page edited in
    place on a double click was planned here and did not make it: it is the
    host's gesture, and is open under "Found by use".)* Taken in four parts,
    each with its commit:
    - ✅ **X5.5.1 - The menu bar.** *(Done 2026-10-05. `score::menu` is the
      window's `menu` value: File (the paper, the orientation), Edit, View,
      Notes, Notation, Measures and Transform. An entry that edits carries the
      editor's verb as its verb, so a pick is read by what reads a method
      call; the window's own -- undo and redo, select all, the layout, entry,
      the value in hand -- is a word, and undo is handed to the context to
      walk. The bar is sent again with every correction, since what it checks
      is the editor's state. The measure verbs it needed are new: `measures`
      (insert before or after, remove), `barline`, `break` and `meter`, each
      over the measures the selection covers. Not in it yet: Save, which
      waits for the score's file verb (`X5.8`); Play, for its playback
      (`X5.8`); Text and the entries that ask for a parameter, for the dialog
      (`X5.5.4`); cut, copy and paste, which have no operation.)*
    - ✅ **X5.5.2 - The toolbar and the input state.** *(Done 2026-10-05.
      `score::tools` is a row over the page: the value, its dot, a rest, the
      accidental, four articulations, a tie, a triplet, the voice, and the
      layout past a spring. The crate names the tools and the caller numbers
      them, so a window opened with none has no toolbar. A tool that holds
      state reports its value and one that acts its `"click"` -- a button's
      value rises and falls with the hand and is a control signal, not a
      command. The **input state** is the editor's: the value, `dotted`,
      `rest`, and an accidental **armed** for the next note and let go once it
      is written; with notes selected the accidental tool is theirs instead.
      Writing a note and its armed accidental is one entry. New verbs:
      `accidental`, and `voice` to a named one; both clients gained them, the
      input state as properties, and the methods of `X5.5.1`'s measure verbs,
      which had none. The tools are labelled in text until `X5.5.3`. Not in
      it: the transport controls, which wait for playback (`X5.8`).)*
    - ✅ **X5.5.3 - The toolbar's icons**, the engraver's SMuFL outlines.
      *(Done 2026-10-05. **Where the outlines come from** was measured: the
      engraver has no door that hands a glyph out and its data directory is
      not reachable in a page, but it draws any codepoint asked of it as a
      notehead (`head.shape`), as an outline, in both builds. So
      `notation::specimen` writes one note per codepoint, `Score::outlines`
      engraves it, reads the glyph table every page carries and puts the
      document back -- two loads, once per editor, and the same call native
      and wasm. Nineteen symbols: the seven note values, the dot, a rest, five
      accidentals, four articulations and the triplet's figure. **How the host
      draws one** keeps its rule that an icon is a glyph of the font: a
      `window` may carry `glyphs`, a codepoint to the path of its shape, and
      the font module draws a character that has one as a shape in the glyph's
      cell, as its own symbol set is -- so a tool's label is one character and
      no widget learned a picture prop. A tool whose symbol the engraver did
      not hand out keeps its text label; the tie has none in the font and
      stays text. The size an outline is drawn at (the em 1.25 body boxes
      tall) is set by reasoning and is to be judged by eye.)*
    - ✅ **X5.5.4 - A dialog while the window is up.** *(Done 2026-10-05.
      An editor answers with props and never with nodes, and a dialog is a
      node; so the three forms are **in the window from the start**, each
      alone on a page of a `stack` that shows none, and opening one is the
      stack's `index` corrected to its page with its fields corrected to what
      the score holds. The host needed nothing: a hidden page is not placed,
      and a modal `layout` on a placed one is set aside and centred as any is
      -- pinned by a layout test and written into the protocol's reference.
      `score::dialogs` holds the forms -- the page's text (the header's
      fields, and the footnotes one to a line), the page's margins and staff
      in millimetres, and the number a transformation takes (transpose,
      stretch, repeat) -- and `editor/forms.rs` what each does: a field
      reports its text as it is typed and the editor keeps it, `OK` writes
      one entry, a form that cannot be read stays up with the reason, and
      `Cancel`, Escape and the close mark take it down. The menu gained the
      entries, disabled in a window composed without the dialogs' widgets.
      **A text of the page edited in place is not in it** and is recorded
      under "Found by use": it needs the page to report a double click and to
      say where the text is drawn, which is a gesture of the host's.)*
  - ✅ **X5.6 - The palettes.** The ● entries first, as titled, collapsible
    groups. Each ○ entry is the model growing an item or a field with its
    emission, its reading and its `Op`; they are taken palette by palette,
    each its own step, and the ones not taken when this milestone closes are
    written down as open. *(Done 2026-10-05, for the ● entries.
    `score::palettes` is a column beside the page, a divider between them:
    Notes (a triplet, the two grace notes, the other voice), Accidentals,
    Articulations (the thirteen MEI names), Ornaments, Lines (slur, tie, the
    two hairpins), Dynamics (`pp` to `ff`, `sf`, `fp`) and Measures (insert,
    remove, the barlines, the breaks), which opens folded. A group is a titled
    section that folds on its strip, the host's to remember; an entry is a
    flat button labelled with the engraver's symbol where it has one, whose
    tip says what the element is in a sentence, and whose click is one of the
    editor's verbs over the selection. The entries are named by the crate and
    numbered by the caller, as the tools are -- the three sets of chrome ids
    are now one `Chrome`. The grace note had a field in the model and no
    verb: `grace` is new, in the crate and both clients. **No ○ entry was
    taken**: each is the model growing, and they stay as the list above
    writes them -- `multiRest`, the tremolos, `ornam`, `arpeg`, `gliss`,
    `breath` and `caesura`, `lv`, `phrase`, `octave`, `pedal`, `bracketSpan`,
    `beamSpan`, `tempo`, `dir`, `reh`, `fing`, `harm`, lyrics, a change of
    key or of clef inside the score, `ending`, `repeatMark`, `mRpt` and
    `beatRpt`, a staff's line count and label, and `staffGrp` -- all grown
    since, under "Found by use".)*
  - ✅ **X5.7 - The notation keys.** The table above settled key by key:
    the written pitch, the written value and the tuplet, `staff` and `voice`,
    the mark against the level, the grace note, the spanners over event ids,
    and the sequence's notation section. In the core, reserved, read by both
    clients, with its reference page in the books. *(Done 2026-10-05.
    **The spellings were the user's**: flat keys on the event, under the
    model's own names -- `pitches` for the written pitch, `value` for the
    written value -- "it can be changed later; the development has a long way
    to go and compatibility can break further on". Weighed and left: a prefix
    (`written_pitch`), MEI's `pname` (which is the letter alone there), every
    key nested under one, and an event type of its own for notation, which put
    `staff` and `voice` on an event at another level than the item's.
    `clausters_core::event::notation::KEYS` is the table: twelve keys, each
    with what it holds, its unit and which way it is read. `pitches`, `value`,
    `staff` and `voice` are new and reserved; no tuplet key, since an exact
    value says one. Two keys are the written form of what the event also
    sounds, and the table says the source: an event that states no `freq`,
    `midinote` or `degree` sounds the first of its `pitches`, in the render
    and in each client's `midinote()` alike (`written_midinote`, bound); a
    chord is one event per note. On the way to a page a slot takes `pitches`
    and `value` as they are -- an F flat and a triplet eighth now reach the
    sheet -- and reads `staff` and `voice` past. A mark is the source and a
    level its performance; a grace note takes no time of the bar. What is no
    note's is the sequence's **`notation` section**
    (`EventSequence::notation`), kept as written and read by nothing a
    sequence does, where what has two ends names them by event id. Each
    client's `NOTATION_KEYS` is the table's names, pinned by a test, and both
    books carry the table row for row, pinned too. Core ABI 82.)*
  - ✅ **X5.8 - The score into a sequence, and its playback.** *(Taken in
    four parts: `X5.8.1` the render and its curves, done 2026-10-05 and
    described under this entry; `X5.8.2` the score's file; `X5.8.3` its
    playback; `X5.8.4` the export.)* The render,
    one way, in Rust, and the Python client's `to_timeline` replaced; the
    interpreter yielding curves in its three scopes, a channel per voice and
    the channel group; saving and reading the score's file;
    `render_events` on the score in both clients, whose
    sequence opens as a roll with its automation; the score's own playback over it on a server transport (as
    `X3.8`'s), with the cursor following, the loop and play from the
    selection; export to `.mid` and to a clip.
    - ✅ **X5.8.1 - The render and its curves.** *(Done 2026-10-05.
      `clausters_document::events::score::render`, behind the crate's
      `notation` feature, bound as `clausters_core_sheet_render_events`: every
      sounding note an event, a chord one per note, with the interpreter's
      sounding keys and the notation keys beside them; a tie chain one event
      of its whole written value; each voice on a channel counted from the
      top. `notation::levels` is the interpreter grown: a staff's dynamics and
      hairpins as a function of time, which says exactly what the attacks say
      at the moments they are read. The render writes it as a controller lane
      (`dynamics_cc`, 11) unless the reading hears a dynamic in the attack
      alone (`dynamics_as`: `attack`, `curve`, `both`, the default -- an
      instrument listens to one or the other, as the two are the same fact).
      **The channel group, decided**: the same lane on each channel of the
      group, each naming the `group` in its target (`"staff 1"`), so what
      plays a lane by its channel learns nothing new and what edits them as
      one finds them by that name; an edit to one of them is an edit to the
      group (the entry under "Found by use"). The
      note's own scope yields nothing: the model holds no glissando and no
      crescendo inside one note. The sequence is MIDI 1.0 while its voices fit sixteen channels.
      What is no note's goes into the `notation` section, the spanners over
      event ids and `items` saying which item each event came from. Both
      clients' `to_timeline` and `to_sequence` are now this render -- the
      Python one written in the client is gone -- and the client `Score` has
      `render_events`. Core ABI 83.)*
    - ✅ **X5.8.2 - The score's file.** *(Done 2026-10-05. **The verbs are
      the ones a buffer has**: `Score.read(path)` and `score.write(path)` in
      both clients, the score remembering its file (`path`). It is written as
      MEI, whole, and read in whatever format the engraver finds it to be.
      **The editor touches no file**: the File menu's Save -- and Ctrl+S,
      which is the window's own -- is a turn whose outcome names the path, and
      its holder writes it: the native host on the disk, a client on the disk
      or in a page's own storage. A score with no file is asked for one by the
      file form, which Save as always opens; Open names a file the holder
      reads and hands back as the `open` verb, which replaces the score as
      one entry, the score that was there a step back. The host's own
      `--save-to` is now that path, given to the editor when it opens.
      `editor.save` and `editor.load` are the two as methods. Not in it: New
      and Close, which the menu's list names and nothing here built then --
      they are under "Found by use".)*
    - ✅ **X5.8.3 - The score's playback.** *(Done 2026-10-05. **The
      editor holds no transport**: a play is a turn whose outcome is the pass
      it asks for -- `from`, where the selection starts or the cursor was
      left; `range`, the stretch several selected items cover; `looping` --
      and a rewind is a `locate`. The window says its owner plays it, so the
      space bar and `L` arrive as its own verbs; the toolbar gained the
      transport past its spring (back to the start, play, the loop switch)
      and the menu a Play menu; the three are one switch and one pass. What
      plays is the score rendered at **the engraver's own tempo**, 120
      quarters a minute, measured: the page's cursor is drawn over the
      engraver's timemap, so the sound and the line agree with no conversion.
      Each client plays that sequence with the playback a notes editor uses
      (`NotesPlayback`: an event lane on a transport of the server's), keeps
      the one sequence and hands it the next render after an edit or a step,
      so both are heard on from where the position is, and binds the window's
      head clock to that transport. `play`, `pause`, `resume`, `stop`,
      `playing` and `transport` are the client's, as a notes editor has them.
      The standalone host has no server and plays nothing (since 2026-10-06
      it does, under "Found by use"). Open: the host's
      own `L` switch and the toolbar's are told apart by nothing -- the
      editor's is the one a play reads, and a press of `L` after the toolbar
      turned it may ask for the state it already has (one switch since
      2026-10-06, under "Found by use"); and a tempo the score
      states is not read, since the model holds none (`tempo`, in the
      palettes' open list -- read since 2026-10-05, a tempo mark setting the
      render's tempo map).)*
    - ✅ **X5.8.4 - The export.** *(Done 2026-10-05. The File menu's
      Export MIDI and Export clip name a file through the file form, and the
      turn's outcome names it and its format (`smf`, a Standard MIDI File;
      `clip`, a MIDI 2.0 Clip File -- which is what "a clip" is here: the
      sequence already writes one). Its holder renders the score and writes
      it as a sequence writes either, so nothing about MIDI is in the editor
      and what is exported is what the roll shows: the notes, a channel to a
      voice, the dynamics as each channel's expression, at the engraver's
      tempo. `editor.export(path, format)` is the two as a method, the format
      read off the extension when left out. **The standalone host exports
      nothing**: it links no MIDI file writer, and says so; the writer is the
      clients' crate (`clausters-midi`), and giving the host the same one is
      what closing that takes -- done 2026-10-06, under "Found by use".)*
  - ✅ **X5.9 - The books and the example**, both clients. *(The user,
    2026-10-05: the editing examples are gathered into one.)* `notation/
    score.py` (a drag and an undo), `notation/score_editor.py` (every verb)
    and `notation/compose.py` (the operators, now the Transform menu) become
    **one example** that opens the application, in both clients; the
    others go with the script-side editor they assembled. `score_from_data.py`
    renders a timeline into a score rather than editing one, and stays. *(Done
    2026-10-05. `notation/score_editor.py` and `notation/score-editor.html`
    are the one example: a document opened and read, operated on with the
    model's operators, opened in the application, a verb called on it from
    the script, the score saved, and its render opened as a roll. The script's
    own rows of buttons are gone *(the user: "quitale los botones del chrome
    viejo que quedaron abajo de todo y no se usan")* -- the window is the
    application's whole. `notation/score.py` and `notation/compose.py`, with
    their pages, went with the script-side editor and the operator walk they
    were. The pair audits call for call. Both books' chapter on composition
    gained a section of its own for the score editor -- the page, the window,
    the verbs, the paper and the page's text, playing, the file, the render
    -- where the steps before this one had each added a sentence to one
    paragraph.)*

  **Open, each decided at its step:** whether a spanner's end can also be
  dragged onto another note, beside re-choosing it from a selection; how a
  channel group is named in the sequence; each notation key's spelling
  (`X5.7`). *(Settled on the way: an accidental's press and drag are its
  note's, since the walk gives a note's parts its id; the hit index is a
  uniform grid; the page view's `breaks` mode is `smart` with its threshold at zero
  (first settled as `auto`, which the first look at the page showed ignores a
  written break -- "Found by use"); pages stack one under
  another; the toolbar's icons are the engraver's SMuFL outlines.)* `N7` (what
  a foreign score's layout keeps) and `N9` (a score as a box of the
  multitrack) stay where they are; `N9`'s double click opens this
  application.

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
  whether a monitor (scope, phase, spectrum, meters) is one application or several;
  and whether the heavy families -- `timeline` and `signal` -- become Cargo
  features a build can drop, as `notation` and `patcher` are *(moved here from
  `clients/gui/PLAN.md` by the user, 2026-10-04, "The heavy families as
  features", which keeps what was measured: `signal` is named in twenty-nine
  files and the timeline family in nineteen, at the frame and the pipelines
  rather than at the edges, so making either a feature is a structural change
  first)*. Which of these widgets become applications decides what a build
  could drop, so the two questions are taken together.

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
  `Playback.stopAtEnd` in both clients. *(Replaced 2026-09-29 by
  `MultitrackPlayback::set_end` and `Playback.end`, three ways -- open, the
  contents, an end marker -- shared with the notes editor: "Where a pass
  ends", Found by use.)*

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

- ⬜ **X9 - The multitrack editor, continued.** *(Opened 2026-10-01 by the
  user, to hold what waits for the multitrack's development to resume: the
  clone "is to be postponed until the multitrack's development continues, in a
  nested X milestone".)* The multitrack editor's milestones so far are the
  document's (`crates/clausters-document/PLAN.md`, `O25`-`O33`) and this
  crate's `X8`; what is gathered here is taken up when that work starts again,
  each part opening on its own decision.

  - ⬜ **X9.1 - A clone: a new sequence made from a clip, or from a segment of
    one.** *(Moved here 2026-10-01 from `clients/python/PLAN.md`, Future
    directions, where it was written; the decision it waits on is
    `crates/clausters-document/PLAN.md`'s `O21`(a) -- whether a region is one
    object or two, the slot and what fills it -- since if it is two, swapping
    what fills a slot while keeping the slot is a copy by construction.)*
    *(named 2026-09-03 by the user, reading the window/copy question above: "crear
    una nueva secuencia a partir de un clip o un segmento de un clip clonado, ahí
    sí el contenido del nuevo clip sería una copia de los datos en otra
    estructura")*. Today the arrangement has one way of making a second thing out
    of a first, and it is a **window**: a split, a trim and a join all refer, and
    nothing is copied. The verb that is missing is the other one, and it is a
    **deliberate act rather than a side effect** -- take this clip, or this stretch
    of it, and give me its contents as **a structure of their own**: a new
    timeline of notes, a new take of samples, independent from that moment on.

    It is worth having on its own terms. A window is right for a cut, and wrong
    for "I want to develop this phrase without touching the one it came from" --
    which today can only be done by writing the timeline out in a script. It is
    also the verb whose absence makes a split *feel* like it should copy, so
    naming it separates the two questions instead of letting one answer both.

    **And it is a candidate answer to the entry above** ("A window onto notes is a
    window while the session lasts and a copy once it is written"): if a split of
    notes *cloned* rather than windowed, each half would hold its own timeline
    with its own node ids, nothing would collide, and the crate would need no
    change at all. What that costs is exactly what the windows buy -- the cut
    would stop hiding and start deleting, so dragging a half's edge back out would
    bring back nothing, and a join would no longer be the inverse of a split but a
    merge of two independent copies. The two are therefore read together, and the
    decision is one decision:

    - **windows all the way** -- the crate learns to let a segment name a node,
      the split stays reversible, and the clone is added beside it as its own
      verb;
    - **clone for notes** -- the split over notes copies, the crate is untouched,
      and the reversibility a take has is not a thing a phrase has;
    - **both, with the window as the default** -- which is what a DAW's "make
      unique" does to a shared clip, and is probably where this lands, but it is
      written here as an option rather than chosen.

    Whatever it is, the *verb* belongs in the vocabulary in one shape for every
    contents, like the others: a clone of a stretch of samples is a bounce of that
    window into a new take, which is the same act one unit over.

    *Re-read 2026-09-14: still missing, in today's terms.* A piece is regions
    windowing sources, and a join mints a source made of segments; every verb
    refers and none copies. The clone is still the other verb — consolidate a
    region, or a stretch of one, into a source of its own (a bounce, for samples;
    a copied timeline, for notes) — and the choice it names between windows and
    copies is now a choice about sources rather than about `form`'s tree.

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

- ⬜ **A roll may need its tempo and its barlines as parameters** *(the
  user, 2026-10-06, hearing the page of a sequence against the rolls over
  it)*. A sequence carries its tempo map as data and a roll reads it; one that
  states none is played and drawn at a beat a second, and nothing on `edit` or
  in the roll's window says another. And a roll rules its grid in beats: it
  has no meter, so it draws no barline, while the page of the same sequence is
  barred. Open, and the user's to say which was meant: the tempo and the meter
  as options of `edit` for a sequence that states neither, controls of the
  roll's window that edit what the sequence holds (its map, and the meter of
  its `notation` section, which a page already reads), or both -- and what the
  page of that sequence writes, since an engraver counts 120 quarters a minute
  where a score states no tempo and a page says its tempo with a mark.
  *(The user, the same day: the bars may stand on the roll's ruler, where it
  already counts them -- but the meter has to be able to change along the
  roll as it does on a page. So what the ruler reads is not one meter but the
  page's own grid, its meters by measure, which the sequence's `notation`
  section holds and a reading already takes.)*

- ✅ **From the roll to the score** *(the user, 2026-10-05, planning `X5`:
  the conversion to a roll goes one way, and the way back takes decisions
  that can be made later)*. `X5.8` renders a score into an `EventSequence`;
  reading one into a `Sheet` is the other direction, and each part of it is a
  decision: snapping onsets to written values (and to tuplets), spelling
  pitches the events do not spell, finding voices and staves, reading curves
  back as dynamics and hairpins (a hand-drawn curve is no hairpin), and what a
  sequence the roll edited means to the score that rendered it -- whether the
  score is re-read from it, or the edit stays the sequence's. With it, `edit`
  opens a sequence in the score editor (`view`, the presentation a sequence
  has beside the roll), including one built from a client's plain numbers.
  The notation keys (`X5.7`) are what makes the exact case exact: a sequence
  whose events carry them is read back with nothing to decide.

  **Decided 2026-10-06, with the user, and taken with the entry below and
  with `N9` (`clients/gui/PLAN.md`) as one sequence of work:**

  - **The sequence is the structure, and the page is a reading of it.** The
    reading is a function in Rust, beside the render
    (`clausters_document::events::score`), that decides only what the events
    do not say; it replaces the reduction each client wrote
    (`sheet_from_timeline`, `sheetFromTimeline`). **It never rewrites the
    sequence**: what was played from a keyboard keeps the times and the
    lengths that arrived, and how finely it is snapped is a parameter of the
    reading, changed without touching the data. The meter is given, not
    found, and tuplets are asked for: finding a meter or an irregular value
    in a performance takes an algorithm this does not attempt.
  - **An edit on the page changes in the sequence only what it changed on
    the page.** The page is rendered before the edit and after it, and an
    event the two renders agree on is a note the edit did not touch: its
    event in the sequence stays as it was, with its time, its level and its
    curves. A note the edit touched takes the render's event, notation keys
    and all, so what was decided on the page persists in the sequence. The
    other answer -- the sequence replaced whole by the render -- was weighed
    and left: the first edit on the page would snap every note of a
    performance.
  - **A curve is a note's or a channel's, and that is the criterion both
    ways**, whatever MIDI specification the sequence names: a note's curve is
    a note's mark (a bend straight to the next note a glissando, a level
    moving inside the note a hairpin over it), a channel's is its voice's (the
    dynamics, the hairpins, the pedal), and a curve that is not the shape a
    mark renders as stays a curve of the sequence. In MIDI 1.0 and 2.0 a
    channel says the voice; in MPE a channel is a note's, so the zone is the
    part and its voices are found.
  - A score held as itself is as it was: saved as MEI, and `render_events`
    one way.

  **The steps:** the reading, bound in both clients as the score's
  constructor from a sequence; `edit(sequence, view="score")` with the edit
  written back as above; then `N9`. *(All of it done 2026-10-06; `N9` is
  built and closes in its own plan with its eye pass. What each step left
  open is listed under it.)*

  - ✅ **The reading.** *(Done 2026-10-06.
    `clausters_document::events::transcription::read`, behind the crate's
    `notation` feature and bound as `clausters_core_sheet_read_events`: a
    sequence and a `Transcription` -- the meter, the key, the clef, the beat,
    the smallest value (`division`, a sixteenth), the tuplets admitted, the
    most voices on a staff, whether levels are read -- answered as the sheet
    and which item each event became. An onset is snapped to its beat's grid,
    and a beat is a tuplet's only where one is asked for (or stated by an
    event's `value`) and fits its onsets clearly better, the ends weighing a
    quarter of an onset; a note is cut and tied where such a beat ends, since
    a tuplet fills its beat. A staff is a channel and a voice is found, a
    note that starts under another going to the next; in an MPE zone the
    staff is the zone. A number is spelled by the key -- the signature most
    notes are in where none is given -- on the line of fifths, four under the
    tonic to seven over. A level is read from a curve of steps and straight
    ramps on the dynamics' controller, else from the notes where it changes;
    a line at one level is written with no dynamic. **The exact case**: the
    render's section gained the staves as they are written, `controls` and
    `groups`, and `marks` -- what a note carries that no notation key says --
    so a rendered score reads back the same MEI. `Score.from_events` and
    `sheet_from_events` in both clients are this reading, and the reduction
    each client wrote is deleted; `from_timeline` and `from_notes` are the
    same reading under the names they had, so a chord under a melody is now a
    second voice where it was clamped to one layer, and each note of a chord
    keeps the spelling its own event states. **The emitter changed with
    it**: a run of tuplet values is a group to each written value it fills,
    where it was one group as long as the run, which no barline could cross.
    Core ABI 85.)*
    **Left open by it**, each for when it is met:
    - a level is named by the interpretation's own table, whose amplitudes
      are a synth's: a take's MIDI velocities all fall at its top, so a
      caller reading one hands in a table of its own. It is the entry
      "Dynamics need a model of the instrument and of hearing".
    - a score's repeats are played out in its sequence, so one with repeats
      is read written out, on its first meter alone;
    - a control standing on a rest has no event to name and is not in the
      section;
    - an end that falls in a beat with no onset is snapped to `division`,
      tuplets or not;
    - the keys are the fifteen major signatures, as the model's are.
  - ✅ **`edit(sequence, view="score")`**, the edit written back.
    - ✅ **The write-back.** *(Done 2026-10-06.
      `clausters_document::events::writeback::write_back`: the page rendered
      before the edit and after it, compared item by item. An item both
      render alike leaves its events as the sequence has them; one they
      render differently takes the difference alone -- each key the edit
      changed, written with its family's coherence and the written pitch
      last; a move in time added to where the event was played; its curves
      where they differ. An item only the page after has is new events,
      played by what plays the sequence, and one only the page before had is
      events removed. A curve the two renders write alike stays the
      sequence's own, and one they write differently is the render's, found
      by what it drives and on which channel, whatever it is called. What
      the page says of itself goes into the `notation` section part by part
      where the edit changed it, what has two ends naming the sequence's own
      events; a sequence that had no section gains one only then, and never
      an `items` list, which is what says a sequence was rendered. With it,
      the reading lets an event that states no `staff` take its channel's
      beside others that state theirs.)*
    - ✅ **The editor over a sequence.** *(Done 2026-10-06.
      `ScoreEditor::over` in the crate: the score loaded with the sequence
      read, and from then on an edit of the page written back and recorded
      **as the sequence's** -- its leg is in the events' vocabulary, put back
      by restoring the sequence -- under the sequence's own key, so the page
      and every roll over the sequence are one structure in the order and one
      undo. The score is held while the two agree, so the ids a selection
      names stand across the page's own edits; it is read again when the
      sequence is no longer the one it agreed with -- a step of the history,
      a note moved in a roll, a script's change -- which every verb of the
      editor checks first. `clausters_apps_editing_open_score_over`, core ABI
      86; `edit(sequence, view="score")` in both clients, with the
      transcription's keys as its options, `ScoreEditor.over` the same as a
      constructor, and `view` refused for a presentation a structure does
      not have. What plays there is the sequence itself.)*
      **The page is on the sequence's time** *(found by the user the same
      day, at the first look: the page's line ran twice as fast as the
      rolls')*: an engraver counts a page at 120 quarters a minute where the
      score states no tempo, and what plays over a sequence is the sequence
      on its own map -- a beat a second where it states none. The editor
      takes each time of the page to the beat the page puts it at, and from
      there to where the sequence's map puts that beat.
      **Left open by it**: the standalone host opens no sequence on a page,
      having no door that asks for one.

- ✅ **A sequence as primitive data, by the keys chosen** *(the user,
  2026-10-05, planning `X5`)*. A sequence the score editor or the roll made
  holds everything in its events, and a client wants it back as its own plain
  data -- `[(note, dur), ...]`, or any other set of keys. So the verb chooses
  the keys and yields a list of tuples (or of rows) in that order, in both
  clients, over the events. It waits for `X5.7`, since the keys it chooses
  from are the ones that table settles. Open: what an event without one of the
  chosen keys yields, and whether a chord is one row or several.
  **Done 2026-10-06.** `sequence.to_rows(*keys)` (`toRows(keys)`), over
  `clausters_document::events::rows` through the door the sequence already
  had, so no symbol is new. **A key is read as the event means it** -- a
  `midinote` from a `degree`, a `freq` or `pitches` alone, an `amp` from a
  `velocity`, a `sustain` through `dur` -- `at` and `id` are the event's own,
  and **a key the event does not hold is null**. **A chord is several rows or
  one, by the shape asked for**: a row per event is the sequence as it is
  placed, and `line=True` is one line played back to back, as a pattern
  writes it -- notes that start together one row whose differing keys hold
  lists, `dur` the time to the next row, a silence a row of its own.
  `EventSequence.from_rows` reads either back, which is what makes a list of
  plain numbers a sequence. **With it, by the user the same day**: the voices
  of a score are sequences of their own -- `sequence.separate(by)`, by voice,
  staff or channel, each part with its curves and what the whole says of its
  page -- and the MIDI specification follows one rule
  (`EventSequence::midi_fit`): MIDI 1.0 where no note carries a curve of its
  own, MPE where one does and the sequence is one line, MIDI 2.0 where it is
  several. The render names its sequence by that rule, so a one-voice score
  with a glissando is MPE now, where it was 2.0.

- ⬜ **Time-stretch: an edge that changes the material rather than the window**
  *(moved here from `clients/gui/PLAN.md` by the user, 2026-10-04: it is the
  multitrack application's)*. An edge drag is a **trim** and the material
  stands still; the other answer a multitrack has is to change the material's
  *length* -- resampling it, or stretching it at pitch -- which is a rendering
  on demand (an NRT pass over the source, its result a new take) and not a
  placement edit. The host already draws the picture such a stretch produces
  (`fit` draws a placement's span over the whole of its material). What is
  undecided: which gesture asks for it (a modifier on the edge, or a verb like
  the split's), what the document does with the result (a new source, or a
  source that remembers it was stretched), and whether the drawing shows the
  stretch before the render lands.

- ✅ **The applications' window chrome in standalone** *(recorded 2026-09-24,
  the user: "para poder correr las aplicaciones del host en standalone se va
  a necesitar que la ventana tenga chrome y no está implementado aún")*. A
  standalone host runs the applications with no client beside it, so whatever
  a script would set — a switch, a mode, a setting — has to be reachable from
  the window itself, and today the windows have no chrome for it. The audio
  editor's loop is a key and a status line precisely because there is none
  (X8). What the chrome is, and which of it is the application's rather than
  the widget's, is the design. *(2026-10-04: the elements it is composed from
  exist in the host — a window's menu bar, a context menu, tools, tips, tabs,
  sections and a dialog (`G37`–`G40`, `clients/gui/PLAN.md`; the
  `panels/chrome` example holds one of each). Which entries an application puts
  in them is still this entry's.)*
  *(2026-10-06, the user, with the score editor the one application that has
  chrome: an application run standalone has its chrome, since that is how it
  is handled there; run from a client it is optional -- the entry below -- so
  what this entry designs for each remaining application is the standalone
  window, and a client opening it bare is part of the acceptance.)*

  **Done 2026-10-06** for the audio, multitrack and notes editors, with the
  score editor as the model and the field's menu conventions (the user asked
  for both): **File** (Save where the window writes its work, Close), **Edit**
  (Undo, Redo; Cut, Copy, Paste -- and Paste mixed in the audio editor --,
  Delete; Select all; Split, Join, Quantize in the multitrack and the roll),
  **View** (Zoom to fit; the multitrack's Reset track heights and Compact
  tracks), **Transport** (Play or stop; the multitrack's Pause and Stop; Go to
  start, Go to end; Loop), **Track** in the multitrack (Add track) and **Help**
  (Keyboard shortcuts). A toolbar under it holds the transport -- the host's
  verbs as tools, or the multitrack's own row by name with its clock -- and
  the edit tools. Composed once in `crate::chrome` and dressed onto each
  window with its `main` view; the vocabulary is the one decided in
  `clients/gui/PLAN.md` ("The whole interaction vocabulary is provisional").
  **Bare from a client**: `"chrome": false` in each open request, passed by
  both clients from the editor's `chrome`; the multitrack's transport row then
  stays under it as before. A pick reported as `"menu" <verb>` is read as the
  verb (`Converse::reads_menu_as_verbs`; the score editor keeps reading its
  own). The standalone host numbers the tools of its own windows, which a
  client does on the way out.

- ✅ **An application opens without its chrome from a client** *(asked for by
  the user 2026-10-06, over the score editor, as a rule for every editor once
  it has chrome: it must be possible to run one from a client with no chrome,
  the client editing through the handle; the application is lighter that way;
  the keys are kept; and what matters most in editing a roll or a score from
  a client is reading back the notes or the events a hand edited, to add to
  or process in code)*. **Done 2026-10-06 for the score editor**, where the
  chrome is: `"chrome": false` in the open request (`ScoreEditor::set_bare`)
  opens the page in its scroll and nothing else. The switch is in the crate,
  where the window is composed, so a client passes one option through: a bare
  editor names no tool, palette entry or dialog for the caller to number, and
  composes no menu bar, no status line and no symbol table. **Absent rather
  than hidden** -- the user's condition for hiding was that it cost nothing,
  and what is not composed is not engraved for, sent or corrected: the
  example's window is 3 widgets where the whole one is 233, a def of 26 kB
  where it is 113 kB, and a turn that moved the input state corrects the
  window's `keys` alone rather than every tool and the whole menu bar. The
  keys stay because they were never the menu's: the window's `keys` prop
  names scopes of the host's table. Both clients: `chrome` on `Editor` and on
  `edit` (`chrome=False`, `chrome: false`), on the base class so the option
  is every editor's and an editor with no chrome opens the same either way;
  the default is the whole window. Reading back was already there and is now
  the example's step (`alone()`, the page's *page alone*): the score the
  client holds is the edited one (`sheet`, `render_events`), and `on_change`
  is told once per gesture. What it left is the entry below.

- ✅ **A bare window does not say why a verb was refused** *(left by the
  entry above, 2026-10-06)*. The reason a refused verb has is shown on the
  status line, and a bare window had none: the verb answered `false` and the
  reason reached nothing. A gesture's refusal reached the host's own status
  bar, through the acknowledgement of its stamp; a verb a client calls has no
  stamp, so its answer settled nothing and the host dropped the reason.
  **Decided 2026-10-06, by the user**: the bare window keeps the status line
  the whole window has.
  **Fixed 2026-10-06**: a bare editor composes the status line under the page
  (the page, its scroll and the line), and a verb refused through the handle
  corrects the line with its reason -- in the whole window as well, where the
  same reason was dropped the same way. Pinned in the crate and in both
  clients' tests.

- ⬜ **What a bare window leaves to its handle** *(left by the entry above,
  2026-10-06)*. The reason a verb was refused is on the status line and not on
  the handle, so a client reading the answer cannot tell a refusal it could
  fix from one it could not. **The reason, done 2026-10-08**: the score
  editor's outcome carries it (`Outcome::refused`), and both clients keep it
  as the handle's `refused` -- set by a verb that answers false, cleared by
  one that goes through. Beside it, still waiting for a use rather than for a
  fix: a form's question (the path of a first Ctrl+S) has no window to be
  asked in, and the handle's `save` is the way; the chrome's other parts one
  at a time; and a switch while the window is open, which is the window
  composed again.

- ✅ **A window shows its keys with F1** *(asked for by the user 2026-10-06:
  a window with no chrome shows its shortcuts with F1, in a modal window that
  scrolls, to look them up quickly; the modal closes with its own close mark
  or Escape)*. **Done 2026-10-06, in the host**, where the key table is: `keys`
  is a host verb bound to F1, and it opens a sheet in the popup layer, in the
  middle of the window, under a title strip that ends in its close mark. It
  lists every key in force in the window -- a section per scope the window
  names, the one read first at the top, then the table's own rows -- with a
  chord a scope takes over listed only where it holds; the table's own verbs
  read in words and any other verb as its name. Nothing on it is picked: a
  press anywhere but the close mark is swallowed, the arrows, Home and End
  scroll it, and the close mark and Escape take it down. Every window has it,
  since the table is every window's; a bare one has no menu to show a chord
  beside an entry, which is why it was asked for there.

- ✅ **The multitrack's stop-at-end in standalone: a key, saved in the
  session** *(decided by the user 2026-09-24; out of X8; done 2026-10-07)*. The switch exists
  (`MultitrackPlayback::set_end`, bound in both clients as `Playback.end`); the
  standalone host needs a key that flips it and the session to keep it, so a
  reopened session stops where it stopped before. *(The chrome it waited for
  landed 2026-10-06: the switch can be a Transport entry and a tool beside
  Loop.)*
  *(Done, with two things the user fixed on the way, 2026-10-07. **Where it is
  kept**: a field of the session, `end`, in the three forms a playback's end
  already has -- absent for a pass that rolls on, `"contents"`, a number of
  seconds for an end marker. Not in a view, since it changes what is heard,
  and not in the multitrack, since it is no edit. The type is the document's
  now (`clausters_document::End`), and both clients' `Session` carry the
  field. **An end marker is carried by the contents**: a region placed past
  one takes the marker with it (`End::carried`, applied by the playback at
  each sync), while a marker put inside the contents is an early stop and
  stays. **An end bounds no view**: it is where a pass stops and nothing
  else, sent to the server's transport and read by no window, so the time
  axis scrolls and zooms out past it.
  The switch is the window's verb `stop_at_end`: `Shift+L`, declared by the
  window in a scope of its own of the key table, *Stop at end* in the
  Transport menu and a tool beside Loop. It flips the end between open and
  the contents; the editor keeps no copy, and whoever holds the playback
  flips it there. The standalone host reads the end with the session, tells
  the playback when it makes one, says the switch on the status line and
  writes it on a save, with no server needed; both clients flip their
  playback's `end`, and a script copies it into `Session.end` when it
  saves.)*

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

- ✅ **A roll with no sequence: whether the bare `pianoroll` stays** *(the
  user, 2026-10-01, deleting the bare `multitrack` builder in
  `clients/python/PLAN.md`, `C57.0`; one question of `X6`; decided and done
  2026-10-07)*. Both clients
  still build a `pianoroll` widget by hand, over no `EventSequence`: in
  `editors/pianoroll` and `editors/pianoroll_midi`, and in a column of
  `panels/gestures`. The two editor examples predate the notes editor (`X3`)
  and teach what it no longer is — their docstring had the OSC markers
  edited by hand, which they no longer are (`C57.0` names them `OscMarker`) — and `edit_notes` and
  `edit_midi_file` show the same through the application. Open: whether a
  roll drawn with no sequence has any use (a multitrack's had none: an edit
  has nowhere to live), and so whether the builder and those two examples go,
  as the bare `multitrack` did, or stay as a view.
  *(Decided by the user 2026-10-07: it goes -- "es código viejo que ya no se
  usa así". Removed from both clients in one commit: the `pianoroll` builder
  and its model name `notes`, the curve-point helper only it called, the two
  editor examples and their pages, and the roll column of `panels/gestures`,
  which now shows the table over a ruler and a waveform. The host's `notes`
  element stays, as the notes editor's; a `Source` still carries `notes` and
  `osc` through the generic `node`. What that left open is the entry below.)*

- ✅ **A roll that paints what is played has no door and no example** *(left
  2026-10-07 by the entry above; done the same day)*. The host's roll takes `midi_in`: it opens
  a MIDI port and paints incoming notes into the roll, reported as ordinary
  `"notes"` events. The bare builder was its only named door and
  `editors/pianoroll_midi` its only example, and both are gone; the notes
  editor's window states no `midi_in`. Reaching it now takes the generic
  `node("notes", midi_in=True)`, over no sequence. It belongs with the
  editor's recording (`X3.10`, through `PLAN.md` `T10`): whether a roll that
  records shows the notes as they arrive through this prop, or only the take
  the server wrote.
  *(Done: the door is the notes editor's. `midi_in` is an option of the
  editor -- `edit(sequence, midi_in=True)`, `midiIn` in the web client -- which
  the crate's editor states on its roll and in every correction of it. What the
  host paints comes back as the `"notes"` a hand's edit reports, so a played
  note is an edit of the sequence: in the history, undone with the rest, and
  drawn in every other window over it -- which the bare roll, over no sequence,
  could not say. `editors/edit_notes` opens its first roll that way, in both
  clients. Painting from a script needs no door at all now: a `MidiFunc` that
  adds events to the sequence redraws the roll, which is what the removed
  example did by hand with `/gui_set`. The recording question stays where it
  was, with `X3.10`: this is the host's live input, at the play line, and a
  take the server records is another thing. The host in a page opens no MIDI
  input, so there the option is carried and nothing is painted:
  `clients/gui/PLAN.md`, Found by use, "The host in a page opens no MIDI
  input".)*

- ⬜ **The score editor's editing rules as modules of their own** *(the
  user, 2026-10-06: "the editing rules of the score editor should be
  modularized; they are very specific. It can be a later refactor, coherent
  with the other applications")*. What a press, a key and a verb mean is
  decided in four places today: the host's `score` element (what a press
  reports, by the page's `entry` and the modifiers), the editor's turn (which
  verb a window key is, by the mode), `score::verbs` (what a verb does to the
  items selected) and `score::selection` (what a selection is, and what a
  verb means over something that is no item). The last two are modules and
  pure; the first two are arms of a `match` inside a widget and inside the
  editor. The refactor names each rule once -- the context a gesture is read
  in, the meaning it has there -- in a module the editor consults, shaped as
  the other applications' are, so the notes editor's and the multitrack's
  rules can be read against it. Open: where the press's rule lives, since the
  host answers a press before any owner hears it (the core's `admits` table
  is the precedent: one table every renderer reads).

- ⬜ **What a score editor's hand expects and this one has not** *(listed
  2026-10-06, settling "A press on a score means what its context says" in
  `clients/gui/PLAN.md` against the field's conventions)*. Three, none of
  them a fix:
  - **The signs the engraver draws from the grid and the staff** -- a clef, a
    key signature, a time signature, a barline, a measure repeat -- are
    selected under ids the engraver mints, which name nothing in the model:
    no verb reaches them, and Delete over a change of clef does not take it
    back. They are written from the score definition and not as elements of
    their own, so naming them is a decision about how the changes are
    written.
  - **A sweep over blank paper selects what it covers.** A press on paper
    lets the selection go and a drag there does nothing; the field's editors
    select by a rectangle.
  - **A menu on what is pressed**, with the verbs that are that element's.
    The host has the menu bar and no menu at the pointer.

- ⬜ **Dynamics need a model of the instrument and of hearing** *(the user,
  2026-10-06, listening to the score editor's hairpins: what can be done now
  is a first reading, and the rest is a direction)*. A crescendo is now
  straight in **amplitude** from its first note's onset to its last one's
  (the entry "A hairpin's level" under "Found by use"), and that is a
  stand-in. What a performer means by it is straight in **loudness**, and
  loudness is not amplitude in decibels alone: it depends on the register,
  the spectrum, the duration and the instrument, so a perceptually even
  crescendo needs a perceptual model and the instrument's, not a curve
  through the level. What waits on those models:
  - **Inside the note or between notes is the instrument's.** A sustained
    sound (a bowed string, a wind, a voice) grows inside a held note; a
    struck or plucked one (a piano) only from one attack to the next. Today
    the reading says it once for the whole score (`dynamics_as`: `attack`,
    `curve`, `both`), and a held note's hairpin curve follows it; it belongs to each staff's
    instrument, which the model does not hold.
  - **Accents are thought apart from the level**, above all on an
    instrument whose amplitude is controlled continuously: an accent on a
    note inside a crescendo is an event of its own over the line, not a
    factor of the note's amp that its hairpin curve then scales from the attack on,
    which is what it is now.
  - **One fact in two places.** With `both`, a MIDI port hears the level in
    the attack's velocity and again in the expression controller, so an
    instrument that listens to both hears it twice; which part of the level
    each carries is a question of what the instrument does with each.
  - A dynamic written while a note is held (in another voice of its staff)
    does not move that note; whether it should is the instrument's too.
  - **The metric accent is a model of its own, built on these**: the
    interpretive one, which reads a meter, a phrase and a style, and is left
    as it is for now (a downbeat's attack times 1.2). Heard in the listening
    pass (the user, 2026-10-06): after a crescendo that ends on no dynamic,
    the next bar's accented downbeat falls back at once, so a following
    *subito p* sounds as if a decrescendo led into it; and inside a
    crescendo over quarters, the downbeat's hairpin curve starts from its accented
    attack and the next note attacks lower, a small dip in the line. Both are
    the accent as a factor of the note's amp rather than an event over the
    level.

- ⬜ **A glissando that arrives without a new attack** *(decided by the
  user, 2026-10-06, after the listening pass over glissandos: a glissando
  often means legato -- a trombone's -- and the page can say so without an
  instrumental or interpretive model)*. The notation is Xenakis': the note a
  glissando arrives at is written with its **notehead in parentheses**,
  which says it is not articulated again. MEI and verovio have it:
  `note@head.mod="paren"`, which verovio draws (and reads from MusicXML's
  `notehead parentheses`); the project takes that name and invents none.
  - **The rule of interpretation**: a note reached by a glissando whose head
    is in parentheses is not attacked. It joins the event of the note the
    glissando leaves, as a tied note does: one event for the two written
    lengths, its pitch curve moving over the first and holding at the
    arrival over the second -- one note-on, its pitch bend and one note-off
    in MIDI -- and both items name that event in the render's `items`.
  - **Glissandos chain**: notes written each with a glissando into the next,
    every arrival in parentheses, sound as one continuous line -- one event
    over the whole chain, its curve moving through every pitch in turn, as
    a tie chain is one sound however many notes it has.
  - **The palette** takes parentheses for an accidental as well as for a
    notehead: verovio encloses an accidental too (`accid@enclose="paren"`),
    the cautionary accidental's sign, so the entry is one family of two
    marks.
  - What it needs, all of it the model's growing as the ○ entries did: a
    field of the note's marks (`head_mod`) and one of its accidental's
    enclosure, their emission and reading, an `Op`, the palette entries, the
    interpreter's join, the render's items, the helpers in both clients and
    their tests.

- ⬜ **A box of notes chooses its synth** *(noted 2026-10-06, closing "A
  track's gain automation does not reach its notes")*. What plays a box is
  what its events name -- their `instrument`, the server's `default` when
  they name none -- so one sequence on two tracks sounds the same on both,
  and changing a track's instrument means rewriting its notes. A box, or
  its track, naming the def its notes play with (and the events' own key
  winning or not) is the instrument track every multitrack has; it needs a
  field of the document, a control in the header and a rule for the notes
  that already name one.

## Found by use: the running list of fixes

Every entry carries a checkbox, and a fixed one stays with the record of what was
wrong.

- ✅ **A box of notes stretched from the front loses where its notes start**
  *(the user, 2026-10-06, at the first look at a box drawn as its page: with
  the box moved, its notes do not sound as they should -- and then the cause:
  stretched from the front, the box loses the relation to where its notes
  start)*. The document and what plays were right: a trim moves the box's
  edge and its window's start together, and the notes were placed at their
  own seconds. The picture was not. Each box's notes, and its page's anchors,
  were stated **from the box's edge**, and a trim is answered with an
  acknowledgement alone, so the host went on drawing them from the new edge:
  they travelled with it, and the picture stopped being what was heard.
  **Fixed 2026-10-06** by stating a box's notes as a take's samples are
  stated, in **the sequence's own frames**, and drawing them through the
  box's window (its `start`, and `rates`): a trim leaves every note where it
  was played, in the picture the moment the edge moves, and a note that
  starts before the window is not drawn, as it is not heard.
- ✅ **The notes editor's playback does the transport's work** *(found
  2026-09-28, writing `X3.9`'s sound; the user: "Pianoroll con midi/osc events
  debería correr con el transport del servidor, de lo contrario estaríamos
  duplicando funcionalidad")*. `X3.8` stamps every event on the transport's
  clock (`/sched_atTransport`) and plans again from the crate on every play,
  locate and edit, with the client querying the clock first: a second
  implementation of what the transport does for a take, and wrong across a
  loop, since the clock does not wrap. A roll holds concrete data as a clip of
  audio does, so it plays on the server's transport: `PLAN.md`, `T8`, the
  event lane, which the notes editor and the multitrack's notes regions move
  onto. `clausters_editing::notes_playback` is what goes.
  **Fixed 2026-09-28 by `T8`**: the playback is an event lane on its
  transport, an edit is the lane's new data (`update`), and nothing asks for
  the transport's clock.

- ✅ **A roll's edit made every note new** *(the user's log of `edit_notes`,
  2026-09-29: the notes came back with ids #53 to #56, each with a
  `velocity` and an amplitude the author never wrote)*. The crate corrects a
  roll with `notes` and `note_ids`, in a `serde_json` map that sorts its
  keys, so `note_ids` reached the host first and the `notes` behind it
  cleared the ids it had just been given. The next report named every note
  0, as one the hand made, and each gesture rewrote the sequence as new
  events that had lost what the roll cannot draw.
  **Fixed 2026-09-29** in the host's `notes` element: the ids set last are
  kept for a list that arrives after them, applied when the lengths agree and
  consumed there, so a list is named by the ids beside it whichever came
  first.

- ✅ **A roll's OSC markers were sent to the server** *(the same log: "lane
  1001: event 3 was not built: /mark cannot be scheduled in a timed bundle",
  on every edit)*. An OSC event is a message to another application, which
  the server cannot send, and `notes_playback::data` wrote it as a lane
  message. **Fixed 2026-09-29**: a lane's data holds the notes only; the
  lane's own `messages` are commands for the server, and a sequence holds
  none.

- ✅ **Where a pass ends, and the space bar** *(the user, 2026-09-29, trying
  `edit_notes`: the roll should be able to play to the last note's end or
  roll on as the multitrack does, better the second; "Para el multipista se
  puede aplicar la misma regla que el roll para el final de lo que existe o
  se puede utilizar un marcador de fin"; and "la barra sea play y stop para
  que el cabezal de reproducción vuelva a posicionarse sobre el cabezal de
  posición")*. The notes editor marked its sequence's end on every play, the
  multitrack had a switch, and the space bar paused both.
  **Done 2026-09-29**: `clausters_editing::playback::End` -- open (the
  default), the contents, or an end marker -- is how both playbacks say where
  a pass ends (`Playback.end` and `NotesEditor.end` in both clients; core ABI
  77), and the space bar is play/stop over both windows, a stop going back to
  the position cursor. The end marker is a number from a script; drawing it
  and placing it by hand is open.

- ✅ **A roll navigates only the window it opened on, and a note in hertz
  grows with the zoom** *(the user, 2026-09-29, trying `edit_notes`: the roll
  should scroll to octaves outside its range as the piano widget does, MIDI 0
  to 127 and hertz past MIDI 127, opening on the contents with a margin; and
  "al hacer zoom in vertical con la representación frecuencial no cambie el
  ancho de las cajas", since a box's frequency is at the middle of its height
  and nothing says so)*. The crate sent the window fitted to the notes as the
  roll's compass, so nothing past it could be reached; and a note was a
  semitone row high in hertz too.
  **Fixed 2026-09-29**: the compass is the domain's whole range (MIDI 0 to
  127; the hertz domain from MIDI note 0 to 20 kHz) and the fitted window is
  the slice the view opens on (`axes.y.start`/`len`); a wheel over the
  keyboard scrolls the window, a tenth of it a notch, and Ctrl and the wheel
  zoom it (`OnAxis::wheel_pans_y`, the roll's alone). In hertz a note is a
  bar of a fixed height with a line at its centre (`pianoroll::note_height`).
  Home and End place the cursor over a roll and a multitrack too.

- ✅ **A time range over a roll and a multitrack** *(the user, 2026-09-29:
  "Al multipista y el roll hay que agregarle la capacidad de poder marcar un
  rango temporal como en el editor de audio", with a modifier since the plain
  drag selects notes and boxes, and anywhere on the view; Alt chosen, an Alt
  click still toggling, and the range played as the audio editor plays
  one)*. **Done 2026-09-29**: the host's `range` gesture step sweeps the
  time range alone and hands a press that never moved to the element as a
  click; a roll and a multitrack give it Alt. The range is kept by the
  editor (beats for a roll, seconds for a multitrack) and the space bar plays
  it as the audio editor does -- from its start to its end, back to the
  position cursor, and the loop switch `L` over it or over everything
  (`play_pass` on both playbacks, `clausters_editing_playback_play_pass`;
  `play(range=, looping=)` in both clients). The standalone host plays the
  same, and learns from the engine when a pass stopped on its end mark.

- ✅ **Two windows of one role over one structure draw on one widget**
  *(found 2026-09-28, extending `edit_notes` with a roll in hertz beside the
  one in MIDI notes: "gui_def: widget id 1000 already in use, skipping")*. A
  view names its widgets by `(structure, role, key)` in the core's registry
  (`Application.id_for`), deliberately without the drawer, so that two views
  of one thing agree about which widget draws which part of it. Two editors
  opened over the same structure -- `edit(seq)` twice, which the history
  supports, one pile per context -- then ask for the same id, and the host
  skips the second window's widget. The hertz roll is keyed by its axis, so
  the pair in `edit_notes` works; two rolls in one axis still collide, and so
  would any editor opened twice over one structure, since every view names
  its widgets through that one door. Open: whether the window belongs in the name,
  or a second editor over a structure is refused and hands back the first.
  **Decided 2026-10-06, by the user**: the first is handed back.
  **Fixed 2026-10-06**: `edit` over a structure with an editor of its kind
  open on it -- for a roll, over the same axis, so a roll in hertz beside one
  in MIDI notes is still two windows -- answers that editor and opens nothing,
  found among the views of the structure's editing context, in both clients.
  A closed editor is not handed back, and a timeline is rendered into a new
  sequence by each call, so it never finds one.

- ✅ **The roll has no play cursor, and its ruler places nothing** *(the
  user, 2026-09-28, trying `X3.9`: "El roll no tiene cursor de reproducción
  o no funciona, no se puede posicionar, no hay cursor que avance con el
  tiempo")*. `X3.7` and `X3.8` gave the notes editor a window and a playback
  and never joined the two: the roll's axis anchored no playhead, the editor
  asked the host for no clock, and a click on the ruler reached the crate as
  a tag it did not read, so no cursor was kept and a play always started
  from the top. The audio editor had all three.
  **Fixed 2026-09-28**: the roll anchors its playhead at 0 and the editor
  asks for the notes transport's position when it opens (both clients, and
  the standalone host's roll window); the ruler's `locate` is the outcome's
  `locate`, a beat of the sequence, which the editor keeps as its cursor and
  cues a stopped transport on (`NotesPlayback::cue`). The standalone host's
  roll also plays now -- the space bar, a cue, an edit heard from either
  window -- where before it drew and edited only.

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

- ✅ **A looping selection does not follow a new selection** *(the user,
  2026-09-25)*. Redrawing the selection while it loops leaves the loop on
  the old span. It was never otherwise in the audio editor opened from a
  client: the live follow (`transport_follows_selection`) is the host
  monitor's, and it acts only in a host that owns the transport -- a
  standalone session (`bin/clausters-gui.rs`) -- while the editor's
  `selection` arm, before X7 and after it, keeps the span for the next play
  and tells the playback nothing. It has to answer a new selection, while a
  loop plays, with the playback's `set_loop`, bound for both clients; and
  in a window that plays itself the host's sweep should not touch the
  monitor's loop.
  *(Fixed 2026-10-07. The editor answers a `selection` with the span a loop
  would repeat over -- the selection, or the take with none, as `space`
  says -- and the `locate` a sweep is let go with, which lands on the
  selection's start, with the same span and `place`; both clients hand it
  to the playback's `follow` verb, which moves the loop from where the
  transport stands and, with `place`, locates the head on the span's first
  frame. That placing is not in the entry above and is why `set_loop` alone
  was not enough: a transport past its loop's end runs on and never wraps,
  so a span drawn behind the head would have left the take playing on
  outside it. The playback acts only for the file in focus, rolling, on a
  pass that loops, which it now remembers from its last `play` or `pass`;
  a pass that runs to its end keeps the end it was played with. In the
  host, `transport_follows_selection` stays out of a window whose owner
  plays it. The start is matched within what single precision keeps of a
  frame: `clients/gui/PLAN.md`, "A selection is reported in single
  precision".)*

- ✅ **A roll over a rendering was offered the drag it could not keep**
  *(found 2026-09-27, writing the notes editor's window over the catalogue's
  roll)*. `catalogue::pianoroll` marked a roll that cannot be edited with
  `notes_editable: false`, which is the **clip's** prop for the notes layer
  inside a box; the `notes` element standing on its own reads `editable`, and
  read nothing, so a roll over what a generator produced offered every drag
  and the owner unwound it after. Fixed the same day: the catalogue writes
  `editable`.

- ✅ **Each editor writes the conversation's turn again** *(audit 2026-09-25,
  with the server's)*. `audio/editor.rs::event` and `multitrack/editor.rs::event`
  build the same conversation `Message` from the event and map
  `Turn::{Nothing, Closed, Step, Stale, Route}` onto an `Outcome` with
  `conversation::answer`, differing only in who owns a widget, the resync and
  the route. The score editor would be a third copy: one
  `conversation::turn(event, version, window, owns, route, resync)`.

  **Fixed 2026-09-27, before the notes editor would have been the third copy
  (`X3.4`).** `turn::turn` reads the message and answers every turn but the
  gesture, which it hands to the editor through the `Converse` trait: the
  conversation, the window, who owns a widget, the resync, the route, and a
  window verb answered before the conversation (the audio editor's save and
  play). `turn::Turned` fills the fields every outcome shares. Both editors
  are the one call now.

- ✅ **A roll's edit sends every note at its velocity's amplitude** *(found
  2026-09-27 by the user, by ear, in `editors/edit_notes` with its timeline
  looping: after one drag the whole melody came back much louder and the high
  note saturated the speakers)*. The server's own log (`RUST_LOG=clausters::osc=trace`)
  settles what reached it: before the edit every note arrived with the
  author's `amp` (0.1, and 0.4 on the last one); after it, **every** note --
  not only the one that was moved -- arrived as `amp 0.78740156`, `velocity 100`,
  which is 100/127, about +18 dB. Nothing was duplicated: one `/synth_new` and
  one `gate 0` per note, from one pass.

  So the write-back rebuilt every note from what the roll can say (start,
  length, pitch, velocity, channel) and the amplitude the author wrote was
  replaced by the roll's velocity. Where exactly is not pinned down: an offline
  replay of `NotesDomain.project` with a hand-built `notes` payload, before and
  after the timeline had played, kept every `amp`, so the difference is in what
  the live host reports, in the crate's `events` reading of it, or in how an
  `Event` turns a `velocity` into an `amp` -- one of the three seams this
  milestone exists to settle. It is left for `X3` on purpose: patching today's
  `NotesDomain`/`NotesEditor` would be work on the part it replaces.

  **What it blocks:** `C54`'s by-ear acceptance over `editors/edit_notes`
  (`clients/python/PLAN.md`), since any edit in that window changes the level of
  what the pass plays. The pass itself was checked without the roll -- edits
  from another thread, and the server's output recorded.

  **Fixed 2026-09-27, in the host.** The document's catalogue holds a roll's
  quintuples as `Vec<f64>` and serializes the velocity as `12.0`; the host read
  it with `as_i64()`, which refuses a float, and fell to its default of 100 --
  on every note, and the channel to 0 the same way. So the roll drew every note
  at 100 and the first drag reported 100 for all of them, and the crate's
  intake, seeing each velocity differ from the one it projected (0.1 * 127 ->
  12), wrote `velocity 100` and its amplitude onto every note. The host now
  reads both as numbers and rounds them; the intake was right, and compares the
  integer it projected with the integer that comes back, so an untouched note
  keeps the amplitude its author wrote.


- ✅ **The graphs a note plays in are never freed** *(found 2026-09-30,
  writing `X3.11c`)*. A channel's graph and each note graph are named by what
  they hold (`clausters_core::event_graph`), so every new shape -- a curve
  added, a note started with another control -- is a new def sent to the
  server, and none is ever freed: they accumulate for as long as the server
  runs, and on a server with a data directory they are persisted, so they
  outlive it. What is missing is giving a def back once no instance and no
  channel graph names it (`/def_free`), and deciding whether generated defs
  are persisted at all. **Related:** "A GraphDef carries the buffers it
  plays" (root `PLAN.md`, Future directions): if a def keeps its buffers,
  persisting one persists them too. **Decided and fixed 2026-10-01** with the
  user: generated defs are **not persisted**, and whoever generates one frees
  it. Every generated name begins with `tmp_`, the server's existing mark of a
  def it never writes (`tmp_ev.` for the curves' readers and graphs,
  `tmp_mt.` for the mixer); a note or channel graph is named for the lane it
  plays from (`CurvePlan::scoped`, the lane's node id), so two playbacks on
  one server never share one; and `NoteCurves` frees, with `/def_free`, every
  graph no instance still alive plays, on each pass and at teardown. The four
  readers and the mixer's defs are a bounded set every playback shares, and
  are only kept out of the store.

- ✅ **A curve added while a sequence plays cuts its channel** *(found
  2026-09-30, writing `X3.11c`)*. A slot's members are fixed by its def, so a
  channel whose set of shapes or of curves changes is a new graph, and its
  instance is made again: the notes sounding in the old one stop at once,
  releases included. An edit of a curve's points does not do this -- a table
  is replaced under its reader -- only one that changes which curves there
  are. Keeping the old instance until its notes end, and adding the new one
  beside it, would make the change as seamless as a point dragged. **Fixed
  2026-09-30** that way: `NoteCurves` makes the new instance under the
  channel's next generation and keeps the old one -- its notes' releases are
  the lane's, sent to their own nodes -- with every table it may read, until
  the next pass gives both back; a channel whose last curve goes finishes its
  notes the same way.

- ✅ **A note cannot be written into the second voice** *(found 2026-10-05,
  building `X5.5.2`'s voice tool)*. The page's `insert` gesture names the
  item the new note **follows**, and an insertion after an item goes into
  that item's voice and adds its time to it, so there is no press that writes
  a second line against music already there. The voice tool is therefore the
  **selection's**: it shows the voice of what is selected and moves it
  (`voice`), which is how two lines written as one come apart, and it is not
  an input state. Writing into a voice directly needs the gesture to name a
  **moment** on the staff -- the measure and the onset the press fell at --
  and an operation that writes there without moving what the other voice
  holds. *(2026-10-05: that is the edit cursor of "Writing notes has no
  cursor, and a note written pushes the rest along", below -- a place in a
  voice, and an entry that replaces there. This closes with it.)*
  **Fixed 2026-10-05 with note entry**: `Ctrl+Alt+2` puts the edit cursor in
  the second voice, and what is entered there is written against the first,
  which does not move; the voice is made, and padded with a rest to where the
  entry starts. The bars past a second voice's end are left empty rather than
  given a rest each (`<mSpace/>`), which is what the page drew the first time
  one was written.

- ✅ **The value and dot tools do not act on the selection** *(found
  2026-10-05, the same step)*. They are the input state alone: with notes
  selected, picking an eighth changes what the next press writes and leaves
  the selection as it was, where the accidental tool, beside them, is the
  selection's when there is one. One rule for the three, or the difference
  said in the tool's tip, is to be decided with the palettes (`X5.6`).
  **Decided 2026-10-05 with `X5.6`**: the toolbar is what a hand writes with
  and the palettes are what is done to what is written. The value, the dot
  and the rest stay the input state, as their tips say; a selection's values
  change by `scale` (Notes, Longer and Shorter), and its accidentals by the
  Accidentals palette, which always acts on the selection. The accidental
  *tool* keeps both readings, since arming one for the next note is its
  reason to be on the toolbar.

- ✅ **A text of the page cannot be edited where it is drawn** *(found
  2026-10-05, closing `X5.5.4`)*. A press on a title selects it and the status
  line says its field and its place; changing it is the page text dialog, or
  `set_text`. Editing it in place needs two things the page does not do: to
  report a **double click** on an element, as against the press that selects
  it, and to hold a text field over the box the text is drawn in, which moves
  with the scroll and the zoom. Both are the host's `score` element's, and the
  field's commit is then the `text` verb that exists.
  **Fixed 2026-10-05**, without the field: the state is the page's own. A
  double click on a text of an `editable` page takes it up, all of it
  selected; the page draws the string as it stands, with its caret and what
  is selected, at the text's own anchor -- so it moves with the scroll and
  the zoom because it is the page drawing it; the keys are a field's (the
  one function both now call); Enter writes it, Escape leaves it, and so does
  going elsewhere -- a press on the page, or the focus leaving it, which the
  host now tells an element (`Element::blur`). What is reported is
  `"text" <id> <string>`, and the editor reads it as the `text` verb. A page
  that edits takes the keyboard focus for this, and says nothing about it.

- ✅ **The dialogs' labels and the toolbar are not laid out by eye yet**
  *(the same day)*. The widths of a form's captions and fields, the height of
  the footnotes' field, and the size an outline glyph is drawn at were set by
  reasoning, with the window not on screen; they are to be judged in the eye
  pass that closes `X5`.
  **Done 2026-10-05**, looking at the window in a browser at the user's
  display factor (1.33), with a system face and with the built-in one. A
  form's captions are 170 wide (the longest, "Bottom margin (mm)", was cut at
  150), its fields 340, its rows carry no margin of their own -- the pitch
  went from 50 to 28 -- and the footnotes' field holds four lines. The
  toolbar's symbols are drawn at `tools::SYMBOL_SIZE` and the palettes' at
  `palettes::SYMBOL_SIZE`; see the entry on the symbols, below.

- ✅ **Two lines of the page's head run into each other** *(found
  2026-10-05, the first look at the page)*. In the example the subtitle,
  centred under the title, and the composer, right-aligned on nearly the
  same line, overlap where the one ends and the other begins. Not yet read
  in the code; the first thing to check is whether the cells were sized with
  the engraver's text metrics while the host draws the text in its own,
  wider, face.
  **Fixed 2026-10-05**, and it was not the face's width: the host took the
  font size the engraver names -- the em -- for the height of its own line of
  capitals, so every text of a page was drawn at 1.4 times the size the
  engraver had laid it out for, and two cells it had kept apart ran together.
  The page's text is drawn at 0.7 of the em now
  (`graphics::score::CAP_PER_EM`), and its hit box is as wide as the string
  is in the host's face.

- ✅ **The symbols of the toolbar and the palettes cannot be read** *(the
  same look, and the user's own: "no se entiende bien qué es cada glifo")*.
  Four things, in the toolbar and the palettes alike:
  - **They are drawn too small.** An outline is drawn in the body text's
    cell, which is right for a letter and small for an articulation dot, an
    accent or an accidental.
  - **At that size they deform.** The toolbar's sharp is missing one of its
    lines: a stroke thinner than a pixel is lost, or two fall on one. An
    outline wants a size at which its thinnest stroke is a pixel, and
    placement on the pixel grid.
  - **Some are text where a symbol exists.** The two grace notes of Notes
    show their name cut to a letter and an ellipsis, in a cell one glyph
    wide: the engraver handed no outline for their codepoint, so the label is
    the fallback. The slur and the tie of Lines, and the toolbar's tie, are
    words as well.
  - **Some have no symbol at all**: the barlines of Measures, and the voice.
  **Fixed 2026-10-05**, each of the four:
  - *Size.* A symbol's size is its text's, so a tool that shows one sets
    `text_size`: 2.5 on the toolbar (an em of 22 logical pixels) and 3.5 in
    the palettes (30). What made them look small was as much the words: the
    host's default text was capitals as high as the whole line, half again a
    desktop's own text. It is three quarters of the line now, for every
    window (`clients/gui/PLAN.md`, "Found by use").
  - *Deformation.* The host keeps a stroke a pixel wide: under the size where
    a music font's uprights are a pixel it draws the outline's edges over the
    fill. And an outline steps by its own width, so a dynamic of two letters
    is not run into its neighbour or cut to nothing.
  - *Words.* `score::icons` draws what the engraver's face does not hold,
    and completes the table the window carries: the grace notes (the face's
    eighth, smaller, with its stroke), the slur, the tie, the hairpins.
  - *No symbol.* The same module: the barlines and the repeats (under SMuFL's
    own codepoints, so a face that has them wins), the voice, the measures'
    three verbs, the breaks, "none", and the transport's back-to-start --
    these under codepoints of the editor's own, in a private plane.
  The palettes' column scrolls, which showed up as soon as the symbols had a
  size: its last groups were under the window's foot.
  Left as they are: the voice selector and the layout selector of the
  toolbar are words (`v1`, `v2`; `page`, `line`) -- symbols since
  2026-10-06, the entry below.

- ✅ **An edit to a sounding pitch does not rewrite the written one** *(found
  2026-10-05, closing `X5.7`)*. The pitch family is coherent -- moving a note's
  `midinote` in a roll rewrites the `freq` and the `degree` it holds -- and
  `pitches` is outside it: an event that holds both keeps the written pitch it
  had after the note was dragged, so the page and the sound part ways. The
  family's rule would be to respell `pitches` by `spelling` when a sounding key
  is set, and to set `midinote` when `pitches` is. It waits on the way back
  from the roll to the score ("From the roll to the score"), which is what
  reads a sequence edited as numbers.
  **Fixed 2026-10-06**, by the family's rule alone: setting a sounding key
  on an event that holds `pitches` spells them again by `spelling` (by whole
  octaves they keep their letters and a forced sign), and setting `pitches`
  on an event that states a sounding pitch moves its `midinote`. What a
  sequence edited in the roll means to the score that rendered it is still
  "From the roll to the score".

- ✅ **The score's page was in no window** *(found 2026-10-05, the user's
  first look at `notation/score_editor`: "No se ve, ni puede crear, ni abrir
  la partitura")*. The crate wrote the container the page sits in as
  `"type": "scroll"`, a name the wire dropped when it took the model's
  (`plane`). The host builds a type it does not know as nothing and a leaf
  carries no children, so the page was never placed -- while every message
  to its id was still answered by the editor, which is why the host's tests
  (they deliver to the id) and both clients' (they read the tree) all
  passed. Fixed by the name, and pinned where the two meet: the host's test
  of the window it opens now refuses a tree with an unknown type in it and
  asks that the page be found in the window.

- ✅ **A written system break was laid out as if it were not there** *(the
  same look)*. The page view asked the engraver for `breaks: auto`, on a note
  saying it had been measured to honour a written break; it does not --
  the example's break before the fifth of eight bars was drawn before the
  seventh. `smart` honours one only on a system at least `breaksSmartSb`
  full (two thirds by default: a break after one bar of four is ignored), so
  the page view is `smart` with that threshold at zero, pinned against the
  engraver in `clausters-notation`.

- ✅ **A written page break does not turn the page** *(the same day, measured
  while fixing the entry above)*. The engraver's system cast-off reads `sb`
  and has no visit for `pb`: under `auto` and `smart` a page break is neither
  a new page nor a new system, and only `encoded` honours it -- the mode that
  breaks nowhere else. So Measures, Break, Page writes a `pb` the page view
  does not show. Either the engraver is taught it (a `VisitPb` beside
  `VisitSb`, carried as a patch of the vendored build) or the break is
  written as something the cast-off does read. **Decided 2026-10-05, by the
  user: the engraver is not patched** -- reading `pb` only when the encoding
  is followed whole is its design, not a defect. The page view honours the
  break on its own side of the engraver (for instance by casting off the
  stretch between two page breaks as its own run of pages); which way is
  settled when it is taken.
  **Fixed 2026-10-05, that way**: a score with page breaks is written as one
  document per run (`notation::sheet_to_mei_pages`), each engraved on its own,
  and the page view stacks their pages, numbered as the whole score's
  (`docs/decisions.md`, "A page break is honoured by laying the score out in
  runs"). Checked in the browser: a break before the third bar of the example
  leaves two on the first page and opens the second, numbered 2, with the
  third. **Left open**: a slur or a hairpin that crosses a page break is drawn
  in neither run.
  **Fixed 2026-10-06**: a line across a page break is written once in each
  run it is in -- to the end of the run it starts in, through one it passes,
  from the start of the one it ends in -- each end a beat of the run's own
  measure, which the engraver matches and draws open at the edge as it does
  across a system break. A slur, a hairpin, a phrase mark, a bracket and an
  octave line are; pinned against the engraver. Still open: a glissando and a
  beam across a barline, which are lines between two notes and have no end a
  page could stand in for.

- ✅ **The example's page had no play cursor** *(the same look: "Tampoco se
  ve el cursor de reproducción")*. Two causes. The example opened the editor
  on the ambient host, which stands alone: the cursor is the position of the
  score's transport, and only a host that is a client of the server reads
  one. The page had `session.gui()` and the script did not; closing `X5.9`
  the pair was made alike by taking the call out of the page, the wrong way
  round. Both have it now, as the notes editor's examples do, and the books
  say which host draws a cursor. And the line itself ran from the title into
  the next system: the room it takes past a system was 0.6 of the *system's*
  height, a staff's stem room on one staff and three times it on a grand
  staff; it is a measure of the staff now (`notation::cursors::stem_room`).

- ✅ **Writing notes has no cursor, and a note written pushes the rest
  along** *(found 2026-10-05, the user's first sitting at the page: "Tiene que
  haber cursor de edición ... Las alturas que se ingresan no pueden desplazar
  a las demás ni dentro del compás ni entre compases")*. Today a press on
  empty staff, with `entry` on, calls `notation::edit::insert`, whose own doc
  says what is wrong with it here: "everything after it moves later by its
  value". So a note written into finished music shifts every note after it
  in the bar and re-bars the ones after that; the place it goes is wherever
  the pointer fell, after the nearest item; and the same press selects and
  drags when it lands on a note, so writing and moving are one gesture told
  apart by a pixel. What it is to be is the settled principle of notation
  programs:
  - **Note entry is a mode with a cursor.** Entering it puts an **edit
    cursor** on a staff, at a time and in a voice, drawn on the page; pitches
    are entered *at the cursor*, and outside the mode nothing is written.
    The cursor is a second line beside the play cursor, and is the host's
    `score` element's to draw from a place the editor names.
  - **An entry replaces; it never adds time.** A pitch entered over a
    **rest** makes a note of the value in hand there, and what is left of the
    rest stays a rest. Nothing after the cursor moves, in the bar or across
    bars: the bars keep their length and every other note its time.
  - **A pitch entered where a note already is joins it as a chord**, rather
    than replacing it or pushing it.
  - **Outside the mode a press selects and a drag moves**, as now; inside
    it a press is an entry and never a drag.

  **Decided 2026-10-05**, by the user, before it is taken:
  - **A value longer than what is left of the bar goes on into the next
    one**: it replaces the positions it reaches there, and the two parts are
    joined by a tie. The bar never grows.
  - **A chord is built two ways**: a press on another line or space where
    the note is, or a pitch entered from the keyboard with a modifier key
    held.
  - **Pitches are entered from the keyboard too, and the keyboard moves the
    cursor.**
  - **The window opens outside the mode.**
  - **The mode is entered with a key or with a button of the toolbar.**
  - **The cursor is put where the selection is**: entering the mode with a
    note or a rest selected puts it on that note or rest, and with a staff
    selected at the staff's start.
  - **The cursor goes when the mode is left, and playing leaves the mode**:
    a play switches note entry off, and the cursor goes with it.
  - **`insert` stays, as its own verb, and it moves everything after it.**
    It is for measures above all (inserting bars) and may serve for notes;
    it is never what an entry does.
  - **The cursor is how the second voice is written**: it stands in a voice,
    and an entry replaces in that voice alone -- which closes "A note cannot
    be written into the second voice", above, when this is built.

  **Decided later the same day**, by the user:
  - **With nothing selected, the cursor goes where it was last left**, and
    to the first beat of the score when it has not been anywhere yet.
  - **The pitches are the letters `a` to `g`.**
  - **The cursor advances by the value after each entry**, and the arrows
    take it back or on.
  - **`N` switches the mode on and off, and `Escape` switches it off**, as
    is the convention (the toolbar's button beside them).
  - **Every key of note entry is a binding**, set by file like the rest of
    the host's key table (`[gui.keys]`, `--keys`), never spelled in an
    element.

  **The default keys, agreed the same day** (each a binding like the rest):
  - **`Shift` + a pitch letter adds that pitch to the chord** at the cursor,
    which then does not advance.
  - **`Left`/`Right` move a note or rest at a time, `Ctrl` + `Left`/`Right`
    a bar at a time, `Alt` + `Up`/`Down` to the staff above or below.**
  - **`Up`/`Down` move the note just entered a step, `Ctrl` + `Up`/`Down` an
    octave**; they do not move the cursor.
  - **`Ctrl` + `Alt` + `1` to `4` move the cursor to that voice.**
  - **The digits pick the value in hand.**
  - **A letter writes its pitch in the octave nearest the note before it.**

  Still to settle when it is taken: the letters are bindings already (`E` is
  `split` in the host's table), and inside the mode its keys go first, so
  the table grows a binding scoped to a mode rather than a second name for
  `E`. What it
  touches: the model gains the verb an entry is (a write over a stretch of a
  voice, where `insert` adds time), bound in both clients; the host's `score`
  element draws the cursor and reads a press by the mode; the example's
  prose and both books' paragraph on a gesture change with it.
  **Built 2026-10-05**, as decided. The model's verb is `enter`
  (`notation::edit::enter`, `Op::Enter`, in both clients): a stretch of one
  voice written over, at a time or at the time an item starts, or a pitch
  added to the chord there. The crate's `score::entry` keeps the cursor's
  arithmetic, and the editor the mode, the cursor, where it was left and the
  note just written. The host's `score` element reads a plain press on a staff
  in the mode as `"enter" <column> <position> <staff>` -- the element whose
  column it fell in -- and draws the cursor the editor names (`edit_cursor`);
  the key table grew **scopes** (`[gui.keys.score]`, `[gui.keys.note_entry]`, a
  window's `keys`), read first in a window that names them, so the letters are
  pitches in the mode and `E` is still `split` elsewhere. The toolbar has the
  pencil, the menu's entry is Note entry, and the window opens outside the
  mode. Checked in the browser: entering, chords by key and by press, the
  second voice, the arrows, Escape. Left as found: overwriting the note a slur
  or a hairpin starts on takes the spanner with it, as a deleted note does.

- ✅ **The ○ entries are notation the model does not hold** *(listed by
  `X5.6`; taken 2026-10-05)*. Each is the model growing an item or a field,
  with its emission, its reading, its `Op` and its palette entry.
  **Decided 2026-10-05, by the user: notation and sound** -- what an entry
  means to a performance is the render's as well: a tempo mark sets the
  tempo, the pedal is controller 64, an octave line moves the sounding
  pitch, a tremolo is its repeated notes, an arpeggio staggers its chord, a
  glissando is a curve, and repeats and endings are played out.
  **Built 2026-10-05**, every ○ entry of the list above: the model's marks
  (a tremolo, a roll, a breath or a caesura, let it ring, a beat repeat, a
  fingering, a chord symbol, lyrics), its spanners (a phrase mark, a
  glissando, the pedal, the four octave lines, a bracket, a beam across a
  barline, two notes alternating), what is written at a point (`Control`: a
  tempo mark, a direction, a rehearsal mark), the grid's changes of key,
  endings, navigation marks, measure repeats and numbered rests, a staff's
  changes of clef, lines, names and transposition, and the groups of staves
  -- each written and read through the engraver's own document, a verb of the
  palettes (three new groups: Text, Repeats and jumps, Keys and clefs, and
  Staves), a method in both clients and an operation in both shells. The
  interpreter plays them (`notation::performance`), the render carries the
  tempo map, the pedal lane and a glissando's bend, and the page's cursor is
  drawn over the same reading (`docs/decisions.md`, "What a page writes is
  what it plays"). Checked in the browser, each drawn as the engraver draws
  it. **Left open**: the sound, which nobody has listened to; let it ring
  drawn under nothing visible on a note with a caesura; a slur or a hairpin
  into a measure that becomes a repeat is dropped with the items the repeat
  replaces.
  **Fixed 2026-10-06, the last**: what is written at an item of a measure
  drawn as a repeat -- a line's end, the pedal, a tempo, a direction, a
  rehearsal mark -- is written at its beat of that measure, the sign standing
  where the item's element would be, and read back onto the item the measure
  holds there. Pinned in the model and against the engraver.
  **Fixed 2026-10-06, let it ring**: the caesura was where it was seen and
  not the cause. The tie was written to the next item, and the engraver
  draws one only when it ends inside its own measure and starts on a note:
  so it drew none on the last note of a measure -- where a caesura stands --
  none on the last note of a voice, and none on a chord. It is now written
  as what it is (the user, the same day: "a tie that leaves the notehead and
  goes nowhere; a rest after the note is the common case"; "one never
  arrives at a note"): from each notehead of the item's last written part
  to a beat of that measure -- a beat on, or half the way to the next note
  or the barline where either is nearer. Pinned against the engraver.

- ✅ **A channel group is not edited as one** *(`X5.8.1`: "editing a group
  as one is not built")*. The render writes a staff's dynamics as the same
  lane on each channel of its group, each naming the group in its target;
  an edit to one of them in the roll changes that channel alone.
  **Fixed 2026-10-06**: the sequence's edit to a lane gives every other lane
  of its group -- the same target but for the channel -- the edited lane's
  points, and removing one removes the group.

- ✅ **New and Close are in the File menu and do nothing** *(`X5.8.2`)*.
  **Decided 2026-10-05, by the user**: New replaces the score in the same
  window, as Open does and as one entry of the history -- one staff, treble
  clef, common time, C major, four empty bars; Close closes the window, and
  with changes not saved asks first: save, do not save, or cancel.
  **Fixed 2026-10-06**: both are the File menu's. What its file holds is the
  document the editor last opened, saved or made new, so an undo back to it
  is nothing to save; a holder that wrote the file a save named says so
  (`saved`). Close with something to lose opens a form -- Don't save,
  Cancel, Save -- and a Save with no file asks for one first; the turn names
  the close (`close`), and its holder closes the window once the file is
  written. An Open now makes the file it read the score's. The window's own
  close mark still closes without asking.

- ✅ **The host's `L` and the toolbar's loop are two switches** *(left
  open by `X5.8.3`: "a press of `L` after the toolbar turned it may ask for
  the state it already has")*. The editor's switch was the one a play read
  and the host's the one `L` turned, and nothing told the host when the
  toolbar or the menu turned the editor's.
  **Decided 2026-10-06, by the user**: a set of measures or of notes is
  selected and a play loops it -- "the same behaviour as the other editors,
  in another domain, the notation's".
  **Fixed 2026-10-06**: a turn of the toolbar's switch or of the menu's
  carries `looping` on the window, which is the host's own switch, so the
  three are one and `L` starts from where it stands. And a selection is **a
  stretch or a place**, as a swept range and the position cursor are in the
  other editors: a measure pressed on its staff, a selection extended with
  Shift, everything, or several items picked, is the stretch a play plays
  once and a loop repeats -- from its first note to the end of the one that
  ends last, though the measure holds one note; one item picked is where a
  pass starts.

- ✅ **A window's widgets asked one by one engrave the score once each**
  *(found 2026-10-06, looking at a selection in the browser)*. A client asks
  the editor for a window's widgets one at a time (the door's `props`), and
  the score editor answered each with the whole window corrected -- the page
  engraved again -- whichever widget was asked for. Measured in a page: one
  `select` made from the script engraved the score 165 times, once for every
  tool and palette entry, three loads each with a page break written. A
  press on the page does not take that path, so a hand never saw it.
  **Fixed 2026-10-06**: only the page and its scroll are answered with an
  engraving; the status line is answered with its sentence, a tool with the
  chrome's state. Pinned by counting what the engraver draws.

- ✅ **A run of pages after the first carries the engraver's own credit**
  *(found 2026-10-06, the same look)*. The engraver signs the foot of a
  document's first page where no foot is written, and a run that opens on a
  page break is a document of its own: the score's second page had the
  credit the first one, which has a foot, does not.
  **Fixed 2026-10-06**: a run that does not start the score is written with a
  foot, an empty one where the score has nothing on every page. Pinned
  against the engraver.

- ✅ **The page number stood in the middle of the head** *(the user,
  2026-10-06, looking at a second page: "page numbers do not go at the top in
  the middle")*. The running head wrote it centred, on every page from the
  second.
  **Fixed 2026-10-06**: it stands at the page's outer corner, as a score's
  page numbers do. The head writes it against the right margin, which is the
  outer one of an odd page, and the page view turns an even page's over to
  the left, as far from the paper's left edge as it was from the right --
  where it already writes the number over the engraver's count. Pinned
  against the engraver.

- ✅ **The window's close mark loses unsaved changes without asking**
  *(found 2026-10-06, building Close)*. The File menu's Close asks; the mark
  on the window's frame frees the window at once, since the host closes it
  before any owner hears. Asking there means the host holding a close until
  an owner that wants to ask has answered -- a window prop saying so, and a
  close request the owner confirms -- which is the host's, for every
  editor, and not the score editor's alone.
  **Fixed 2026-10-06**: a window carrying `ask_close` is not closed by its
  close mark, nor by a desktop window's Escape: it reports `close`, as its
  menu bar's entry for it where the bar has one, and its owner frees it once
  nothing is left to lose. A second close the owner has answered nothing
  since closes it anyway, so an owner that stopped answering cannot keep it
  open; and a standalone host whose owner freed its last window ends, as the
  close mark ends it. The score editor's window asks when it has its forms
  to ask in; a bare one closes at once, since the score it edits is the one
  its holder keeps. *(Narrowed the same day, by the user: a window asks only
  where it is the work's one holder -- the entry after next.)*

- ✅ **The standalone session's windows close without asking** *(found
  2026-10-06, after the entry above)*. A window asks only where it is the
  work's one holder, which is a standalone host's, and there only the score
  editor's asked: the multitrack and the rolls edit the session the host
  saves with Ctrl+S, and their windows closed and let unsaved changes go.
  This entry first said it waited for the chrome in standalone; it did not,
  since the close form is a dialog any window holds on a page of its own,
  with no menu bar or toolbar around it. And a roll is not a holder of its
  own: the user, the same day, on an application inside another -- the outer
  one governs, the multitrack here, and when its window closes every window
  that depends on it closes too (`crates/clausters-document/PLAN.md`, "An
  application inside another").
  **Fixed 2026-10-06**: the session knows whether it has changes its file
  does not hold (its history's version against the one last written or
  opened); the multitrack's window holds the close form the closing module
  builds (`closing::stack`), numbered by the host, and says `ask_close`; its
  editor answers the rule like every editor, and the form's Save writes the
  session and closes -- a save that fails says why on the status bar and
  keeps the window. A roll opened from the multitrack depends on its window
  (`Host::depend`), so the multitrack's window takes its rolls with it
  whichever way it closes, and closing a roll alone loses nothing.

- ✅ **Closing and the key sheet are every application's** *(asked for by
  the user 2026-10-06: F1 and the close are a general mechanism, and have
  to be available equally to any application; F1 works with no chrome; and,
  decided the same day, an editor a client holds does not ask at all --
  "the client can close it, and it already has the data in the handle, which
  can open the window again; what matters is that the edited information is
  not lost, and if it is already in the client's objects it is not, and the
  client is responsible")*. The host half was general already; the
  application half was the score editor's alone. **Done 2026-10-06**:
  - **The close is one rule** (`closing`): at once, unless the window is the
    work's one holder and has something to lose, and then the editor's form
    asks. The shared turn reads the window's `close` for every editor; an
    application adds whether it has something to lose, whether its holder
    said the window is the only one (`asksToClose`, which the standalone host
    says and no client), and the form it asks in, which the module builds;
    every editor's outcome carries `close`; both clients' `Editor` and the
    standalone host free the window on any editor's. So a client's score
    editor, whole or bare, closes at once by its mark and by File, Close, and
    the standalone host's asks.
  - **A scope is the application's** (`score::keys`): the window brings its
    scopes, their default chords and the words F1 shows them with (its
    `verbs` prop, bare or whole), and the host lays them into its table under
    what the user bound -- three layers, so a config's
    `[gui.keys.note_entry]` still wins whenever the window opens. The host's
    table names no application now.
- ✅ **The standalone host's score neither plays nor exports**
  *(`X5.8.3`, `X5.8.4`)*. **Decided 2026-10-05, by the user**: with
  `--score` the host boots its embedded server, as `--session` does, and
  plays the score with a simple def of its own; it exports with the MIDI
  writer the clients use.
  **Fixed 2026-10-06**: `--score`, in a build with `standalone`, opens the
  on-demand server and a player as `--session` does. The score's render
  plays as a roll's sequence plays in that host -- the editor's playback on
  a transport of its own, through the player -- and the page's cursor is
  bound to that transport, as a client's score editor binds it; a pass, a
  stop, the loop switch, a rewind and an edit heard on are the turn's, as
  in both clients. The def is the server's built-in `default`, which a
  render's events name by naming none. An export writes the render with
  `clausters-midi`, the clients' writer, which the `score` feature now
  pulls in. Nobody has listened to it.

- ✅ **The toolbar's voice and layout selectors are words** *(the symbols'
  entry, above: `v1`, `v2`; `page`, `line`)*. They take symbols, as every
  other tool has.
  **Fixed 2026-10-06**: the voices are the quarter with its stem up and
  with it down, as a page tells two voices apart -- SMuFL's two, the second
  drawn from the face's first turned over where the face lacks it -- and
  the layout is a sheet with its systems and a system running on, both the
  editor's own. Found with them, by eye: **the latches read as on when
  off.** A toggle drawn as a button took, off, the fill a segmented
  choice's chosen option has, so the toolbar's note entry, dot, rest and
  loop all looked pressed; the host now draws a latch as an option chosen
  or not -- the field's well off, the chosen fill on.

- ✅ **A glissando's pitch, and how it is heard** *(the user, 2026-10-06,
  in the score editor: a glissando must be logarithmic in frequency to sound
  right, or go through midi-to-hertz, depending on how it is built)*.
  Checked, and it already is: a glissando (MEI's `gliss`) renders as the
  note's own curve over its pitch, played as a pitch bend, straight in
  **semitones** from its onset to its release
  (`events::score::render`), and every player turns a pitch bend into a ratio --
  `midiratio` of the channel's and the note's bend on the server
  (`event_graph::pitch_def`), `2^(s/12)` where a client sends the values
  itself (`event_curves`), and a per-note pitch bend in semitones in a MIDI
  2.0 file -- so the frequency moves geometrically: halfway up an octave's
  glissando is a tritone, not the mean of the two frequencies. Two tests hold it
  there (`a_glissando_is_straight_in_pitch_and_geometric_in_frequency`, and
  the render's glissando in semitones).
  **Found while listening, and fixed 2026-10-06**: a glissando written from
  the last note of a tied chain -- where it is written -- was lost, since the
  chain sounds as its first note and the glissando was looked up on that one.
  It is now the chain's, and starts at its last note (`Note::gliss_from`):
  the tied part holds its pitch. The terms were also put back to the
  engraver's, at the user's call (bend and glissando are two notations: a
  bend is a curve out of a stretched string's note, a glissando a straight
  line between orchestral notes): the model and the performance say
  `gliss`, as MEI and verovio do; "pitch bend" names only the MIDI message
  a glissando is played as; and the curve of a held note under a hairpin is
  `hairpin`, not a word of the project's own.

- ✅ **A hairpin's level** *(the user, 2026-10-06, in the score editor: a
  crescendo or a diminuendo grows straight from the note it starts on to the
  one it ends on -- in amplitude for now, from the first note's onset to the
  last one's -- and whether it grows inside a note or between notes is the
  instrument's)*. The attacks were already straight in amplitude over that
  stretch. Two things were not. **A hairpin that ended on no dynamic let
  go**: the notes after it dropped back to the level before it, where the
  level it reached holds until the next dynamic -- the curve and the attacks
  now hold it. **Nothing grew inside a held note on a synth**: the level's
  curve is a controller (11, expression), which a synth does not listen to,
  so a crescendo over a held note was heard only at the next attack. Where
  the reading hears a dynamic in the attack and the curve both (`both`, the
  default), a note held under a hairpin now carries it as its own curve
  over `amp` (`Note::hairpin`, rendered as the note's
  automation), from its level at the attack to where the hairpin has taken
  it at its release, straight between; with `attack` -- a struck sound --
  no note carries it. What a good reading of dynamics needs beyond this is
  "Dynamics need a model of the instrument and of hearing", under Future
  directions.

- ✅ **A track's gain automation does not reach its notes** *(the user,
  2026-10-06, trying the standalone session: the track's envelope -- its
  gain -- has to act on a sequence of notes too, whatever the MIDI
  automations and the velocities say; a track's envelopes apply over them,
  and here they do not act, because one is the audio's and the other the
  MIDI's)*. The notes of every box over a sequence play on the multitrack's
  one event lane, and its voices sound through their own `out`, **outside
  the tracks' strips** (`clausters_editing::playback`, `notes`): the strip is
  where a track's gain, its gain curve, its mute and its meters are, so none
  of them reaches a track of notes -- only mute and solo do, by deciding
  what is placed at all. What a track produces is what its strip shapes,
  whether it reads samples or plays notes: the notes' own velocities and
  note-level curves shape each voice, and the track's envelopes apply after
  them, as over a take.
  *(The same day, the user, on the shape: the box of notes makes its sound
  with the synth it has, and the track takes that sound as its input the
  way it takes a segment of audio -- and the node graph may have to be
  reviewed for it.)* So a box of notes is a **source of sound inside its
  track**, as a box of samples is: a slot of the track's group whose
  output is what the strip reads, where today a box of samples is a slot
  holding readers and a box of notes is nothing of the track's. What the
  review has to settle, against the graph as it is
  (`clausters_document::multitrack::nodes::plan`, `clausters_editing::instance`):
  - **a slot whose contents are voices**: a box's slot holds readers made
    once and kept, and a box of notes holds voices made and freed as the
    transport plays -- whether a slot can be a group the event lane makes
    voices in, writing to the slot's own out, or the voices stay on the
    lane and name the bus of the track they sound into;
  - **one lane or one per box**: the multitrack plays every sequence on one
    event lane in the transport's group; a box's voices sounding into its
    track either splits the lane per box (each made in its slot) or gives
    each voice its box's bus;
  - **the curves inside a note**: a note's own curves are graphs the lane
    makes beside its voices (`clausters_editing::note_curves`), and they go
    where the voices go;
  - **a box moved to another track** carries its sound with it, as a slot of
    samples is moved and re-wired (`/graph_moveSlot`);
  - **the synth**: what plays a box is what its events name (their
    instrument, the server's `default` when they name none) -- a box's own
    choice of synth, beside what its events say, is a design of its own.

  **Built 2026-10-06, and what the review settled.** A box of notes is a
  **clip of its track** over a bus of its own -- the clip's strip, with an
  input where the readers are (`clausters_core::mixer::voices_graph`,
  `VOICE_SLOT`) -- so the box has its own gain, mute and curves and the
  track's strip, its curves and its meter are after it, as after a take.
  - **The voices stay on the lane and name their box's bus.** A slot's
    members are fixed by its def and a note's def is whatever its events
    name, so a voice is not a member of the clip: it is told the bus as its
    `out`, the control an event's `out` key already sets
    (`MultitrackPlayback::notes`; a key of that name on a note of a box is
    not read). The bus is the client's (`Op::AudioBus`), handed to the clip
    on `in0`/`in1`.
  - **One lane**, as before, its notes made in a group **before the
    multitrack** (`instance::VOICES`): a voice has to have written its bus by
    the block the clip reads it. It was the tail of the transport's group,
    after the multitrack.
  - **The curves inside a note** go with the voice: a note's graph has the
    controls it is started with as ports, `out` now among them, and the
    channels' instances are made in the same group.
  - **A box moved to another track** is moved as a clip of samples is
    (`Op::Move`), with the bus it had.
  - **Which boxes play notes is the notes' to say.** The document names a
    source and not what is behind it, so `nodes::plan` cannot know; the
    playback plans each box the placed notes name
    (`nodes::plan_voiced`), against the multitrack the last `sync` was of.
    No door changed, so neither client did.
  - **Mute is the strip's.** `placed_notes` no longer leaves a silenced
    box's notes out: they sound into a muted strip, and a note that began
    under a mute is there when it is lifted. A MIDI message and a command
    are still left out of a silenced box, since no strip reaches them.
  - **Two things the built-in `default` needed**, both the server's: an
    `out` control (it wrote buses 0 and 1 as constants, so an event's `out`
    did nothing on it), and a bus analysis that reads `out + 1` as the
    static index it is -- arithmetic over controls and constants -- rather
    than as a barrier the group's sort cannot cross (`docs/auto-order.md`).
  Heard by a render: `tests/multitrack_playback.rs` (a track's gain curve
  over a plain note and over one its own curves shape) and
  `tests/mixer_graph.rs`.

  **What is left**, each on its own:
  - **A def with no `out` control** sounds where it was written to, past the
    track. It is the event convention's own limit and it is stated in the
    books; nothing reports it.
  - **A MIDI message or a command in a box** goes where it names (a
    channel's binding, a node). Routing a bound instrument into the track of
    the box that plays it is not designed.
  - **The roll's own play** (`NotesPlayback`, a transport of its own) is the
    sequence alone, outside any track. Whether a roll opened from a box
    plays through that box's track is part of
    `crates/clausters-document/PLAN.md`, "An application inside another".
  - **A box's own choice of synth** -- the entry in "Future directions"
    below, "A box of notes chooses its synth".
  - **The clients' builders refuse a channel list on a control's bus**
    (`clients/python/PLAN.md`, Found by use), so a stereo instrument with an
    `out` control is written one `out` per side.

- ✅ **A stereo take plays both its sides on the left** *(found 2026-10-06,
  by a render, writing the test of a box of notes on a track)*. A clip's
  readers are one per channel of its source, and its slot wired every one of
  them to the first channel of the clip's bus: the take's right side was
  summed onto the left and the right was silent before the balance. No test
  held a stereo take -- the mixer's were all over a mono one. **Fixed**: a
  reader is handed both channels of the clip's bus (`out0`, `out1`) and
  writes each at a gain its channel says (`clausters_core::mixer::reader_def`).
  Two buses named by controls rather than one worked out as `out + chan`,
  which is what the audio editor's reader does: a bus index a UGen computes
  made its node a barrier for the group's sort, and inside a track that put
  the meters before the strip they read (`tests/mixer_graph.rs`,
  `a_stereo_take_keeps_its_two_sides`). *(The sort reads that arithmetic
  since the entry above was closed, the same day; the two controls stayed.)*
