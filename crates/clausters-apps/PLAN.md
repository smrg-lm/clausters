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
  document (`crates/clausters-document/PLAN.md`, `O24`). The notation model and
  what is still open about editing a page are the N track's
  (`clients/gui/PLAN.md`: `N7` what opening a foreign score preserves, `N8` which
  element admits which edit, `N9` a score as a box of the multitrack). A `Score` already
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

- ⬜ **The applications' window chrome in standalone** *(recorded 2026-09-24,
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

- ⬜ **The multitrack's stop-at-end in standalone: a key, saved in the
  session** *(decided by the user 2026-09-24; out of X8)*. The switch exists
  (`MultitrackPlayback::set_end`, bound in both clients as `Playback.end`); the
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

- ⬜ **A roll with no sequence: whether the bare `pianoroll` stays** *(the
  user, 2026-10-01, deleting the bare `multitrack` builder in
  `clients/python/PLAN.md`, `C57.0`; one question of `X6`)*. Both clients
  still build a `pianoroll` widget by hand, over no `EventSequence`: in
  `editors/pianoroll` and `editors/pianoroll_midi`, and in a column of
  `panels/gestures`. The two editor examples predate the notes editor (`X3`)
  and teach what it no longer is — their docstring had the OSC markers
  edited by hand, which they no longer are (`C57.0` names them `OscMarker`) — and `edit_notes` and
  `edit_midi_file` show the same through the application. Open: whether a
  roll drawn with no sequence has any use (a multitrack's had none: an edit
  has nowhere to live), and so whether the builder and those two examples go,
  as the bare `multitrack` did, or stay as a view.

## Found by use: the running list of fixes

Every entry carries a checkbox, and a fixed one stays with the record of what was
wrong.

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

- ⬜ **Two windows of one role over one structure draw on one widget**
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

- ⬜ **A looping selection does not follow a new selection** *(the user,
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
