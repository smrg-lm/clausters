# Roadmap — the order the open work is taken in

*Rewritten 2026-08-22, when a sitting with the composer example turned up eight
defects across the host, both clients and the document writer, and left three
findings behind that now lead the phase. Before that: 2026-08-21 (twice: the
second time when the arrangement, the editor, the transport and the engraver all
reached the web client, which is what Phase 1 was ordered for), and the same day
before it, when the phase of things found by use was taken whole — its last entry being a design question rather than a fix.
Before that: 2026-08-20, four times — when the
overview work closed, when the four
entries it had left behind were taken — the join that draws an edge, a page's
zoom while a take records, the shape a zoom asks in, and what a column claims at
a discontinuity — when the web client stopped drawing, and again on the pass
that read the whole `PLAN.md` set against this file looking for open work it
never named (before that: 2026-08-19, when the picture-reads-the-samples phase
closed; twice on 2026-08-18, when the clip's interaction rules and the
real-sizes phase closed; twice on 2026-08-17, against the plans and once the
autonomous editor closed). A rewrite **drops what is done** and reorganizes what
is left: this file is not a record of anything, and the record of what shipped is
the git history and each plan's own checkbox.*

**This file is temporary, and it defines nothing.** It is a working sequence
over pending work that lives, already written, across several `PLAN.md` files —
milestones with their own labels, and entries in a plan's "Found by use" or
"Future directions" lists. A roadmap line says only *when* and *because of
what*; the content, the decisions and the acceptance are read in the plan that
owns it, and if the two disagree the plan wins and this file is stale, which is
the normal way for it to be wrong. When the sequence it holds is exhausted the
file goes away; nothing is ever written here first.

**The destination this order serves**, stated so an entry can be judged against
it rather than against taste: **a functionally complete example of the
arrangement, the document and the GUI together** — a composition built in
Python, drawn as a multitrack editor, edited by hand, heard, undone, redone,
saved and reopened — on a model that is **usable and correct at real sizes**,
not only at an example's.

**Both halves are behind the list now.** The loop runs, a saved piece comes back
as the piece that was saved, the editor is an application with its own
processes, a clip has interaction rules, and the taxes every session paid — an
async command's 100 ms floor, a killed editor's segments, a persisted def that
would not load and could not be named — are paid. The picture reads the samples
it maps instead of copying them, and the summary over them is a file it maps
too; a recording is drawn as it fills, at the frame, on any server and in a page
as well as in a window; and a page zoomed past its summary reads what it is
looking at in the shape that span wants — a finer summary, or the samples where
only they will do — recording or finished, so the browser draws what the native
window draws rather than stopping at a bucket.

**The violation that led this list is paid**: the arrangement, the document and
the editor now exist in both clients, held together by parity suites rather than
by care. What leads it instead is what **using** them turned up — what is left of
the findings the editing pass of 2026-08-22 recorded and did not take — followed
by the surface the port named but did not build, and the destination it was
blocking.
Behind them, one track that is genuinely later. Everything else is in the
section after them, named so that "not scheduled" reads as a decision.

**A larger question was opened while that phase was taken and is deliberately
not in this order**, because it is not work to schedule: what the *second*
document is — the application's, as against the arrangement's — in
`crates/clausters-document/PLAN.md`, Open decisions. Nothing below waits on it,
and it is named here only so its absence reads as a decision too.

Where the work lives:

| Track | File | What it is |
|---|---|---|
| `Ox` | `crates/clausters-document/PLAN.md` | the document: tree, intents, log, session, bindings |
| `Dx`, `Hx`, `Ax`, `Kx`, `Ex`, `Gx` | `clients/gui/PLAN.md` | the GUI host: gestures, undo from the hand, measured layers, the widget API |
| `Cx` | `clients/python/PLAN.md` | the Python client |
| `Wx` | `clients/web/PLAN.md` | the web client |
| `Mx`, `Sx`, `Tx`, `Rx` | `PLAN.md` (root) | the server |

Entries below that carry no label are **plan entries, not milestones** — they
are named by their own title and by the plan that holds them, and a phase that
takes one may well turn it into a milestone there first. **A pointer names the
plan and the section, and quotes the entry's title verbatim**, so it is found by
searching for the title rather than by reading the plan through; the sections
are "Found by use", "Future directions" and, in the document crate, "Open
decisions". If a search comes up empty, this file is stale and the plan is
right — that is the normal failure, not a sign the work vanished.

---

## Phase 1 — the packages move together: the arrangement reaches the web client

*Why first: the rule the project already states, applied to the largest
outstanding violation. **The named track is in** — `src/form/`,
`gui/transport.ts`, `gui/editor.ts`, `gui/notation/` and `defs/patch.ts` all
landed on 2026-08-21, each with a parity suite that asserts the two clients
produce the same tree, the same document and the same engraving. Using it on
2026-08-22 turned up eight defects in one sitting — a curve that drew nothing, a
document written two ways, an edit per frame, a bend that drifted, a press
repeated per frame — and **what leads the phase now is what that sitting did not
finish**, followed by the destination the track was blocking and the one surface
it named on its way past.*

- ⬜ **A page burns the main thread while nothing is happening**
  *(`clients/gui/PLAN.md`, Found by use)* — 67% of the main thread with the
  composer page idle, and the same 67% while playing, which is what makes the
  playhead jitter. It is the browser front's own (native reads the shm header
  and polls nothing), and the entry says to measure per stage before changing
  anything. First because every page the phase adds pays it.
- ⬜ **W16 — Example parity with the Python client**. Its named track (the
  arrangement layer, the editor, the transport, the engraver) is closed; what
  remains is the milestone itself, and it is the larger half: about forty
  Python examples have no page, most of them GUI ones, and each lands with the
  surface it needs rather than as a queue. Two things about the entry's own
  text are stale and are fixed in the plan before it is taken — it asks for a
  **catalog row**, which the project's example rule no longer allows a book to
  hold, and for the **same name**, which the pages already do not keep
  (`gui_composer.py` is `composer.html`).
- ⬜ **`DefPatch` — level 2 of the patcher** *(`clients/web/PLAN.md`, W16)*, the
  one surface the port left named rather than built: a def's internal UGen graph
  as boxes, which `gui_patch2.py` is the example of. A missing surface is a
  defect under the non-divergence rule, so it is ordered here rather than
  waiting for the example that needs it.
- ⬜ **W24 — The completeness pass**, and the parity gaps that plan already
  carries with reasons (the record formatters, `connectGui`, the two leftover
  names — `defs/patch.ts` is no longer among them).
- ⬜ **W7** (the Faust surfaces), **W9** (MIDI), **W15** (the bundle writer) —
  unported features, each owned, none on the path to the complete example.

## Phase 2 — the spectral editor

*Everything here is genuinely later: it needs the A track's descriptors, it is
partly experimental, and none of it is on the path to the complete example.*

- ⬜ **A3 — Band-limited reconstruction and true peak.**
- ⬜ **A4 — K-weighting and the loudness family.**
- ⬜ **A5 — The loudness layer and its read-out.**
- ⬜ **A6 — The layer stack becomes explicit.** It now has a mechanism to be
  explicit *with*: the edit-layer rule shipped general (`host::layers`), and what
  A6 grows is the contents — a view whose automation is a body rather than a
  widget beside it (`clients/gui/PLAN.md`, Future directions, "The layer stack is
  one container's, and an audio editor's view has one too").
- ⬜ **A7 — The layer stack's rules from the clients, and the books.** What is
  left of it once D8 took the client half: it rides with A6, whose rules it
  publishes.
- ⬜ **D5 — Spectral selection.**
- ⬜ **D6 — The lasso.**
- ⬜ **D7 — Spectral drawing and resynthesis** *(experimental: promoted or
  dropped on what it sounds like)*.

---

## Open, and deliberately not in the order above

Named so "not scheduled" reads as a decision rather than an oversight. Each is
in its own plan with its own reasoning; none is on the path to the complete
example, and none blocks anything that is.

- **The GUI host's own documentation** — `K16`/`K16a`/`K16b`/`K16c`. Waits on
  nothing; costs a decision about a fourth mdBook that the project's three-book
  rule does not currently allow.
- **`G31g`** — engraving refinements (tuplets, full polyphony, spelling).
- **Server transport items** `T2`–`T4`, and `R12` (a release verifies
  something). `T2` did not stop being optional when `T5` landed, and the
  question it was flagged with still stands: `T5` put a position in samples on
  the engine, which crosses the beats↔samples conversion `T2` says is anchored
  on the wrong axis.
- **A long take is played out of the pool, and `DiskIn` cannot be positioned**
  *(root `PLAN.md`, Future directions)* — a five-minute stereo take is 110 MB of
  pool and a thirty-minute one 660 MB, per take, and the streaming pair that
  exists for exactly this cannot start anywhere but the beginning. The smallest
  step (a start frame on `DiskIn`) is probably a U-track item; the seek under a
  moving playhead is a design, and the mapped read stays named and unchosen.
  Nothing on the path to the complete example is long enough to force it, which
  is the only reason it is here.
- **The three open questions of the Python plan** *(`clients/python/PLAN.md`,
  Found by use)* — "Acceptable equivalence level for higher math vs Faust",
  "Whether a separate `cdylib` for `clausters-ffi` is preferable" and "The
  FFI-overhead threshold at which the scalar builtin uses a pure-language
  fallback". Each is a number or a decision that nothing currently waits on;
  they are named so that "unanswered" reads as a decision rather than an
  oversight. **`C18`** sits with them, deferred by a decision taken with the
  user: live OS MIDI output stays best-effort, and the UMP-over-our-own-transport
  direction has no date.
- **The builders could be generated from the catalog instead of contrasted
  against it** *(root `PLAN.md`, Future directions)* — the contrast tests caught
  eleven drifted builders, which is strictly weaker than not hand-writing the mirrors.
- **The heavy views' three rendering questions** *(`clients/gui/PLAN.md`, Future
  directions)* — "Cache lifecycle", "Spectrogram scaling" and "Migrating the rest
  of the `Stft` machinery behind `clausters-ffi`/`libclausters`". They lived
  mid-plan and without checkboxes until 2026-08-20; the last of them is the one
  with a dependency (the inverse FFT waits on the server's `FFT`/`IFFT`).
- **The web client's three directions** *(`clients/web/PLAN.md`, Future
  directions)* — "Node target" (true in the harness, not a product), "Type-safe
  GuiDef/def schemas" (narrower than it was: the question is which source
  generates them, not whether one exists) and "A remote-server standalone page"
  (what W4 left is a destination seam in place of the page's singletons).
- **Many channels are drawn and not yet readable, and a take cannot be created
  empty**, **time-stretch: an edge that changes the material rather than the
  window** and **the audio editor's layers: the view is the next container, and
  it is the richer one** — `clients/gui/PLAN.md`, Future directions, being
  designs rather than fixes. The last of those is the one the user asked to have
  thought through and built next, and it is a track's worth: the same layer
  mechanism with richer contents, visualization layers kept separate from edit
  layers.
- **The larger "Future directions"** in each plan — the free arrangement plane,
  an interpreter inside a standalone host, per-node staleness, more than one
  owner of one document, IME text, a Tauri wrapper, the heavy families as
  features, a steady goniometer.

## Revising this file

Reordering is expected and is the point: an entry moves when a dependency turns
out to run the other way, or when something found by use has to go first. What
must not happen is content migrating here — a milestone that grows a decision
grows it **in its plan**, and this file keeps naming it. **A rewrite erases what
has been done**: a closed phase leaves no line here, because the plan's checkbox
and the git history already say it shipped, and the only thing this file is for
is what is still ahead.
