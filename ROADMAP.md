# Roadmap — the order the open work is taken in

*Rewritten 2026-08-24, twice: the second time when the heavy props that are not
samples got the source object, which emptied the reform's list of gaps. Before
that, the same day, when the control's curve and step closed. Before that: 2026-08-23 (four times: the last after the warp family reached the
core and both clients, which put the shared half of the control-range item in
place and turned up three things that had never been written down — a wire
vocabulary with three exceptions in it, a version that lives in five files, and
the range maps' missing signal side. Before that: three times, the third once
the GUI reform's port had landed whole — including the page decision, which turned out to need nothing
from the host that W4 had not already shipped — so what leads Phase 1 is the
gaps the reform wrote down rather than faked and the question about the widgets
it did not ask). Before that: 2026-08-22, when a sitting with the composer example turned
up eight defects across the host, both clients and the document writer. Before
that: 2026-08-21 (twice: the
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
by care. **A new one leads it**, and it is the healthy kind — the reference
client was reformed and the port has not caught up, which is the build strategy
working rather than failing. So the order is: the one decision the reform could
not take from the native side, then the port of the reform, then the two gaps it
wrote down instead of faking, then the examples, and only then the destination
the phase was ordered for.
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
produce the same tree, the same document and the same engraving, and `DefPatch`
closed the one surface it named on its way past.*

***What the warp family left, on 2026-08-23.** SuperCollider's eight range
maps and the exponential-endpoint rule now live once in
`clausters_core::warp`, reached by both clients, and the three copies of that
curve the tree carried — the envelope's exponential shape, its bent one, and
the server's `XLine` — read it instead of restating it. The control's own curve
and step, which that half was built for, closed on 2026-08-24 and are gone from
this list. What the family left behind sits at the end of the phase: it has no
signal side yet, and doing it turned up a wire vocabulary with three exceptions
in it and a version number that lives in five files.*

***The violation the phase created on 2026-08-23 is already paid.** The Python
client's GUI surface was reformed — a builder returns a view that opens itself,
any root opens, ids belong to the instance, the samples are a source, a widget is
built from the control it drives and a window binds against it — and the port
landed the same day rather than being scheduled, which is the build strategy
working as written, page decision and all. Both gaps it wrote down instead of
faking closed on 2026-08-24 — the control's curve and step, and the source for
the heavy props that are not samples — so what is left of it is the question
about the widgets it did not ask.*

- ⬜ **Every carrier takes an address, and all three default to loopback**
  *(`PLAN.md`, Found by use; the host's half in `clients/gui/PLAN.md`)*. Named
  while deciding whether a GUI host may be remote: it may, and the useful
  topology is a bind question rather than a client one. Choosing a carrier is
  not consenting to the network, and today `--tcp` and `--ws` open the port to
  the LAN on both fronts. Here because it touches the server and the host at
  once and nothing above it depends on it — not because it is urgent.

- ⬜ **A typeface is a wasm binding, so only the browser can change one**
  *(`clients/gui/PLAN.md`, Found by use)*. There is no `/gui_*` verb for a
  face: the browser front takes one at runtime through the raw binding and the
  native front only at launch (`--font`). A wasm export grew surface the
  protocol never had, which is the case the non-divergence rule names outright.
  Here rather than earlier because nothing waits on it — but it is the one
  entry of the web book's "What the browser changes" that is not a platform
  difference, so it should not sit unnamed.

- ⬜ **Three wire selectors carry a spelling the table meant to leave behind**
  *(`PLAN.md`, Found by use)*. The operator vocabulary is lowercase and joined
  in seventy places and carries an underscore in three — `as_int`, `as_float`,
  `hypot_apx` — and the plan entry that added the last of them says outright it
  was being lowercased "like every other name in that table". The client half
  is already done (both now spell every operator SuperCollider's way), so only
  the wire lags. Here rather than earlier because it is a **format** change: a
  stored def carries the selector, so the work is choosing between an alias
  arm for a release and taking the break while the major is `0` — and neither
  is decided.

- ⬜ **`LinLin`/`LinExp` as UGens, over the warp family the core already
  carries** *(`PLAN.md`, Future directions)*. The eight range maps are value
  functions in both clients since 2026-08-23; a **signal** still cannot be
  mapped, so a def writes the multiply-and-add by hand — which is two
  implementations of one map, the exact thing the shared core exists to
  prevent. The formulas are done and `warp::apply_map` is the body; what a
  milestone decides is the shape (five- and six-ary, so they cannot join the
  operator tables) and two questions: whether the bounds are modulatable
  inputs or init-rate like a ramp's geometry, and whether `range`/`exprange`
  are UGens at all or the `madd` composition sclang makes them, which reads a
  signal polarity our graph does not track.

- ⬜ **A release bumps one version in five places and four of them are checked
  by nothing** *(`PLAN.md`, Future directions)*. The root `Cargo.toml`, every
  crate under `crates/`, `pyproject.toml`, `package.json` and `clients/gui`'s
  own workspace and lockfile; only the crate↔`package.json` pair is verified,
  and `clients/gui` has drifted before for exactly that reason. The choice is
  between a check that reports the drift and `version.workspace = true` plus a
  writer for the two non-Cargo manifests, which makes it impossible. Here
  because nothing waits on it and it is felt only at a tag — but it is felt
  every time.

- ⬜ **What kind of thing is a button?** *(`clients/python/PLAN.md`, Found by
  use)*. Two gaps and the design question they hang from: `button` is the one
  control widget that takes no def control, so `button(gate)` cannot be written
  and a **trigger** is exactly the case that wants it; `toggle` takes no range,
  though `min`/`max` *are* its two values. Both wait on the question under them
  — an "Accept" in a dialog is not a piano key, press-is-the-event is right for
  an instrument and arguably wrong for a command button, and one element serves
  both today. Here, ahead of the examples, because a pass that rewrites thirty
  of them writes buttons throughout and would bless whichever reading it
  assumed.

- ⬜ **C45 — The examples pass** *(`clients/python/PLAN.md`, the API reform
  track)*. The reform changed how an example is *written* and 30 of the 70
  entries are `gui_*`; the pass rewrites them and organizes them in the same
  sitting (the flat directory is a prefix pretending to be a folder, and the two
  clients do not agree on names). It sits here, ahead of W16, because it decides
  the layout and the names **both** clients then use — porting examples into a
  taxonomy that is about to change is the same work twice.

- ⬜ **W16 — Example parity with the Python client**. Its named track is closed;
  what remains is the milestone itself, and it is the larger half: about forty
  Python examples have no page, most of them GUI ones, and each lands with the
  surface it needs rather than as a queue. Its own text was corrected on
  2026-08-23 — it no longer asks for a catalogue or for the same name, and it
  now states the dependency above.

- ⬜ **W24 — The completeness pass**, and what is left of the parity gaps that
  plan already carries with reasons (the record formatters, the two leftover
  names, `Buffer.fromSamples` against `Buffer.read`).

- ⬜ **W7** (the Faust surfaces), **W9** (MIDI), **W15** (the bundle writer) —
  unported features, each owned, none on the path to the complete example.

- ⬜ **C44 — the inverse direction: a widget inside a def** *(`clients/python/
  PLAN.md`, the API reform track)*, deliberately last and possibly never: it is
  recorded as an analysis with a reservation, since it inverts the dependency
  (`defs` would import `gui`) and autogenerates the one thing `/node_set`
  addresses by.

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
