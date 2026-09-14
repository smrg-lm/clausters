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

- ⬜ **X1 - The audio editor.** *(Requirements stated by the user 2026-09-14.)*
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
    write replaced, which was decided for short strokes — cut and paste over large
    files is not designed.

  **Open, and not decided here:** what goes to memory and what to disk, and at
  which threshold; whether an operation over segments records only the segment
  list rather than samples; where the segment model lives once it is Rust's.

  **Related, each where it is written:** the audio editor as an analysis tool, in
  panes and layers on Sonic Visualiser's shape (`crates/clausters-document/PLAN.md`,
  `O24`); the layer stack (`clients/gui/PLAN.md`, `A5`-`A7`); spectral selection,
  the lasso and spectral drawing (`clients/gui/PLAN.md`, `D5`-`D7`).

- ⬜ **X2 - The buffer editor: drawing a table by hand.** *(Proposed by the user
  2026-09-14, on the samples editor as its model.)* A buffer on the server drawn
  and edited by hand — the manual counterpart of `/buffer_gen`, which computes the
  same tables from a formula. The samples editor already draws into a server
  buffer and writes it with `/buffer_setRange` (`clausters_editing::samples::write_steps`),
  so what differs is the table and what reads it, not the path.

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
  history"); configurable key bindings (`clients/gui/PLAN.md`, `G36`) and the
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
