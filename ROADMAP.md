# Roadmap — the order the open work is taken in

*Rewritten 2026-08-18, twice: once when the clip's interaction rules closed and
again when the real-sizes phase did, taking its six entries with it (and before
that twice on 2026-08-17: against the plans and the last month of history, then
again once the autonomous editor closed). A rewrite **drops what is done** and reorganizes what is left: this file is not a record
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
not only at an example's. Both halves are load-bearing, and most of both are now
behind the list rather than in it: the loop runs, a saved piece comes back as the
piece that was saved, the editor is an application with its own processes, a clip
has interaction rules, and the taxes every session paid — an async command's
100 ms floor, a killed editor's segments, a persisted def that would not load and
could not be named — are paid. **The second half is paid too**: the picture
reads the material it maps instead of copying it, and following a recording
costs the block rather than the take, so a take grows with the sound at the
frame. **Nothing left below is a defect anybody sees** — what remains of the
first phase is a saving, and what follows it is not on the path at all: a port
the destination does not wait on, and a track that is genuinely later.

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

## Phase 1 — the picture reads the material

*What it buys: a picture that reads the material rather than a copy of it. **H7, S20 and H8 have shipped**, and with them the `fills` prop and the last copy — the summary a refresh cloned because the slot drawing it held it (2026-08-19). The copies are gone, a write costs its span, a buffer publishes how far it has been written, and a take is drawn as it fills, at the frame. What is left of the phase is what those deliberately did not infer or half-build.*

- ⬜ **A page cannot fold a streamed overview into the picture it holds** *(`clients/gui/PLAN.md`, Found by use)* — what S20 and H8 left standing, with the reason it was not half-built: it is one door in `clausters-core`, a `Pyramid` taking level-0 buckets at an offset. Its sibling, **a recording's unwritten remainder**, shipped as the `fills` prop. **And the native host is named as a consumer too**, now for the weaker of the two reasons it was named for: the lateness it was found through is fixed (a step costs the step, so the poll runs at the frame), and what stands is that the poll re-derives the summary the stream already carries — a host that subscribed would be told rather than asking. See the plan entry.
- ⬜ **A take's overview could be a file beside its region** *(root `PLAN.md`, Future directions)* — dropped from S20 when the frontier turned out to answer the live question on its own; it buys the opening pass and brings a second writer of derived state.

## Phase 2 — the packages move together: the arrangement reaches the web client

*What it buys: the rule the project already states, applied to the largest
outstanding violation. `form/`, `gui/editor.py`, `gui/transport.py` and
`gui/notation.py` have **no TypeScript counterpart at all** — the whole
arrangement, document and editor layer exists in one client. It waits only on the phase above, and is
still here for the same reason it was last: it is a port, and porting is cheapest once the
layer has stopped moving — which is what the closed phases did to it.*

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
- **Server transport items** `T2`–`T4`, and `R12` (a release verifies
  something). `T2` did not stop being optional when `T5` landed, and the
  question it was flagged with still stands: `T5` put a position in samples on
  the engine, which crosses the beats↔samples conversion `T2` says is anchored
  on the wrong axis.
- **The three open questions of the Python plan** *(`clients/python/PLAN.md`,
  Found by use)* — "Acceptable equivalence level for higher math vs Faust",
  "Whether a separate `cdylib` for `clausters-ffi` is preferable" and "The
  FFI-overhead threshold at which the scalar builtin uses a pure-language
  fallback". Each is a number or a decision that nothing currently waits on;
  they are named so that "unanswered" reads as a decision rather than an
  oversight.
- **The builders could be generated from the catalog instead of contrasted
  against it** *(root `PLAN.md`, Future directions)* — the contrast tests caught
  eleven drifted builders, which is strictly weaker than not hand-writing the mirrors.
- **A take is drawn in amplitude and heard in decibels**; **the other text over
  pictures has no plate yet**; **persistence saves the document, not what the
  user did to it** — `clients/gui/PLAN.md`,
  Found by use, each with its record of what was seen; **many channels are drawn
  and not yet readable, and a take cannot be created empty**, **time-stretch: an
  edge that changes the material rather than the window** and **the audio
  editor's layers: the view is the next container, and it is the richer one**
  are in that plan's Future directions instead, being designs rather than fixes.
  The last of those is the one the user asked to have thought through and built
  next, and it is a track's worth: the same layer mechanism with richer
  contents, visualization layers kept separate from edit layers.
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
