# Roadmap — the order the open work is taken in

*Rewritten 2026-08-18, once the clip's interaction rules closed (and before that
twice on 2026-08-17: against the plans and the last month of history, then again
once the autonomous editor closed). A rewrite
**drops what is done** and reorganizes what is left: this file is not a record
of anything, and the record of what shipped is the git history and each plan's
own checkbox.*

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
not only at an example's. Both halves are load-bearing, and what is left of the
list is now **entirely** the second one: the loop runs, a saved piece comes back
as the piece that was saved, the editor is an application with its own processes
rather than a window on a script's server, and a clip has interaction rules —
one layer edited at a time, an edge that trims a window onto material rather
than stretching a picture.

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

## Phase 1 — the second half of the destination: real sizes

*What it buys: "usable and correct at real sizes". Entries rather than
milestones, in the root plan's **Found by use** list except where marked, none of
them blocking, each of them a tax paid by every session — which is why they are a
phase rather than a list.*

- ⬜ **A finished async command waits up to 100 ms to be reported.** Every
  async command pays it — a `/buffer_alloc`, a `/buffer_read`, a def compile,
  any `wait=True`. **First of the phase, and it came forward from the editor
  phase without being taken**: with the sample traffic gone, this floor is
  exactly what a standalone editor pays per operation, and it no longer hides
  behind batched writes. The fix is a wakeup when a result lands rather than a
  smaller `GC_INTERVAL` for everyone.
- ⬜ **A killed editor leaves its segment and its takes in `/dev/shm`**
  *(`clients/gui/PLAN.md`, Found by use)*. A region is the take's whole size, so
  a few crashes fill a memory filesystem with files nothing can tell live from
  dead. The claim already knows how to spot a stale pid; who sweeps, and whether
  an editor may remove a segment it did not create, is the decision.
- ⬜ **A persisted def that no longer compiles warns on every boot, forever.**
  Seven of them on the author's machine since S17 changed `PlayBuf`'s arity.
- ⬜ **A UGen's trailing inputs could be declared optional, so a def survives
  one growing** *(root `PLAN.md`, **Future directions**)*. The cause behind the
  entry above, and the one that deletes the class rather than the noise.
- ⬜ **`transport_group` takes an id where the rest of the Python client takes a
  node** *(root `PLAN.md`, Found by use, though the fix is the Python
  client's)*. A couple of lines; the TS client's `nodeId(...)` is the shape.
- ⬜ **Ten examples play routines on a clock nobody started, and nothing says
  so** *(`clients/python/PLAN.md`, Found by use)*. Their audible half has never run — a
  mechanical fix each, plus one decision about whether playing onto a stopped
  clock should stay silent at all.
- ⬜ **The by-eye pass the hit-test work is owed** *(`clients/gui/PLAN.md`, the E
  track, inside the closed `E23`)*. `number`, `menu` and `text` answer on the
  field they draw now; the example that shows them (`gui_panel`) has not been
  looked at since. Small, and it is the half of that milestone a test cannot do.

## Phase 2 — the packages move together: the arrangement reaches the web client

*What it buys: the rule the project already states, applied to the largest
outstanding violation. `form/`, `gui/editor.py`, `gui/transport.py` and
`gui/notation.py` have **no TypeScript counterpart at all** — the whole
arrangement, document and editor layer exists in one client. Last of the phases
on the path because it is a port, and porting is cheapest once the layer has
stopped moving, which is what the phases before it do to it.*

- ⬜ **W16 — Example parity with the Python client**, and its named track: the
  arrangement layer and the editor. The shape `gui/notation.py`'s port follows
  was decided on the Python side and is written in the web plan rather than
  re-derived.
- ⬜ **W24 — The completeness pass**, and the parity gaps that plan already
  carries with reasons (the record formatters, `defs/patch.ts`, `connectGui`,
  the two leftover names).
- ⬜ **W7** (the Faust surfaces), **W9** (MIDI), **W15** (the bundle writer) —
  unported features, each owned, none on the path to the complete example.

## Phase 3 — the spectral editor

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
- **The K track's group-aware port** — `keys`, `notes`, `patch` (item 3 of K7).
- **Server transport items** `T2`–`T4`, and `R12` (a release verifies
  something). `T2` did not stop being optional when `T5` landed, and the
  question it was flagged with still stands: `T5` put a position in samples on
  the engine, which crosses the beats↔samples conversion `T2` says is anchored
  on the wrong axis.
- **The builders could be generated from the catalog instead of contrasted
  against it** *(root `PLAN.md`, Future directions)* — the contrast tests caught
  eleven drifted builders, which is strictly weaker than not hand-writing the mirrors.
- **The level body's fade is a guess**; **a take is drawn in amplitude
  and heard in decibels**; **an element's look does not answer for the space it
  is given**; **the other text over pictures has no plate yet**; **persistence
  saves the document, not what the user did to it** — `clients/gui/PLAN.md`,
  Found by use, each with its record of what was seen; **many channels are drawn
  and not yet readable, and a take cannot be created empty**, **time-stretch: an
  edge that changes the material rather than the window** and **the layer stack
  is one container's** are in that plan's Future directions instead, being
  designs rather than fixes — as is **an element reads one thing, so two
  fragments over different material cannot be joined**
  (`clients/python/PLAN.md`, Found by use).
- **A mapped take is still copied into the widget before it is drawn**
  *(`clients/gui/PLAN.md`, Found by use)*. What the editor phase deliberately
  did not do: the round trip is gone, the copy is not. It is a design over
  `waveform`/`peaks` — the pyramid's source, the LOD crossfade, and a browser
  that has no mapping and must keep the owned copy — rather than a change to
  the material module, and nothing is waiting on it.
- **The larger "Future directions"** in each plan — the free arrangement plane,
  the in-page shared-memory path, an interpreter inside a standalone host,
  per-node staleness, more than one owner of one document, IME text, a Tauri
  wrapper, the heavy families as features, a steady goniometer.

## Revising this file

Reordering is expected and is the point: an entry moves when a dependency turns
out to run the other way, or when something found by use has to go first. What
must not happen is content migrating here — a milestone that grows a decision
grows it **in its plan**, and this file keeps naming it. **A rewrite erases what
has been done**: a closed phase leaves no line here, because the plan's checkbox
and the git history already say it shipped, and the only thing this file is for
is what is still ahead.
