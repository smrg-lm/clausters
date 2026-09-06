# The application scope - implementation plan

*Opened 2026-09-06 with the user, on the branch `application-scope`. **This file
is temporary.** It exists so one refactor spanning four packages can be carried
out against a written sequence instead of against memory; it is a record of
nothing. When the branch merges and the work is proven, the file is deleted -
and the last milestone below is what makes that legal, because a decision worth
keeping is not kept here. The `AP` labels are this file's own coordinates and
appear nowhere else; they are not roadmap labels and must not reach a published
doc, a docstring, an example or a comment.*

## What this branch is

**The editor stops being the unit, and the application becomes it.**

Today `clausters.gui.editing.Editor` owns everything global about a session on
screen: the host, the window, the widget-id pool, the acknowledgement, and the
screen state (selection, zoom, layer). What is actually shared by several views
lives one level down, in `Editing` - the context the *data* owns: the history,
the version, the list of views to tell. `FormEditor` is `Editor` plus a document,
and `FormEditing` is `Editing` plus a node index.

That arrangement has three consequences this branch is about:

- a widget id is a **lease**, not an identity, so anything in flight across a
  redraw lands on the wrong widget;
- everything global is written **once per language**, so the two clients carry
  ~5 800 (Python) and ~6 700 (TypeScript) lines of `editing/` that are one
  logic in two spellings;
- there is no layer that means "an application", so a second one - a bundle of
  editable subviews that is not the multitrack - has nowhere to be built, and
  the open question in `crates/clausters-document/PLAN.md` ("What is the second
  document: the application, and not the arrangement?") has nothing to be
  answered against.

The inversion: **an application owns the host, the id space, the acknowledgement,
the screen state and the undo order; an editor is one structure bound to one
`Domain` and one `View`, and owns nothing global.** `FormEditor` is then not a
subclass of `Editor` - it is an *application* whose structure is a document and
whose views are lanes and clips, and it becomes the second consumer of the
abstraction rather than the place it is hidden.

## What is already right, and is not to be re-derived

Stated because half of this refactor is *not* moving things that already sit
well, and a pass over this code will otherwise "fix" them:

- **The inverse is the crate's.** `history::Editable` (`apply`, `current`,
  `coalesce_key`) is bound by both clients; `PointsDomain.current`/`project` go
  through `clausters_domain_edit`, the coalescing key through
  `clausters_domain_coalesce_key`, a curve's drawn axis through
  `clausters_core_curve_axis`. No client re-derives an inverse.
- **The history belongs to the data** (`Ox` O15-O19). `Editing.of` caches the
  context on the structure; `clausters_history_register` mints a structure's
  identity in Rust. Two windows over one thing walk one order.
- **`Domain` and `View` are separate, and the reason is right**: one structure
  is drawn several ways while its vocabulary is one.
- **The builtins are the core's.** `midicps` and the rest reach both clients
  through `clausters_core_unary`/`_binary`/`_map` and the slice forms; nothing
  here changes that, and nothing here should add a second numeric path.
- **`GuiNode` has one construction path.** `clients/gui/src/tree.rs` builds the
  same node the JSON parser produces, deliberately, so a Rust-side projection
  has a door already.

## Non-goals

- **No second owner of a document.** The version counter is enough precisely
  because one owner is authoritative per resource; nothing here moves toward
  operational transformation or CRDTs (`Ox`, "More than one owner of the same
  document").
- **No shared mutable memory as a model.** Shared memory stays what it is: a
  native fast path under an ownership rule that is already decided. The browser
  has no mapping and the web client is first-class, so a design that needs shm
  to be correct is a design that has already diverged.
- **The opaque leaf stays opaque.** Nothing here lets the document interpret a
  generator's configuration. A projection that needs to draw one asks its owner
  for a summary; it never grows a case for `Pbind`.
- **No new widget, no new gesture, no new drawing rule.** This branch moves
  where code lives and what a widget id means. If it grows a visible feature,
  that feature has escaped its own milestone.
- **`clausters.form` keeps its surface.** Users of the arrangement API should
  not be able to tell this happened, except that ids stop appearing.

## The milestones

### AP0 - The seam, in Python only, with nothing moved

Introduce the application/editor boundary where it is cheapest to move it, and
prove it with the editors that already exist before a line of Rust is written.

- A new `clausters.gui.editing.Application` (name provisional until AP8's doc
  pass; it is the object a window set belongs to). It takes over, from `Editor`:
  the host handle and its resolution (`_resolve_host`), the id space, the `Echo`,
  the poll/wait/close surface, and the undo/redo stepping that walks composed
  views (`_step`, `project_legs`, `reflect_step`).
- `Editing` becomes reachable **from** an application as well as from a
  structure: an application has one editing context, and registering a structure
  in it is what an `Editor` does on construction rather than on first edit.
- `Editor` keeps `structure`, `domain`, `view`, the unit bridge, and `_route` /
  `_observe`. It no longer holds a host, mints an id or answers a host.
- `composed_in` / `composed_over` disappear as a mechanism: a composed editor is
  simply an editor registered in the same application.

**Acceptance:** `open_signal`, `open_pianoroll` and a bare `Editor` over a
buffer, a curve and a timeline all work unchanged from a script's point of view;
two windows over one structure still walk one undo order; the existing Python
tests pass with no change to their assertions. No TypeScript in this milestone.

### AP1 - A widget id is derived, not leased

The fix for the recurring visualization failures, and the thing every later
milestone depends on.

- A widget id is **derived** from a stable key rather than taken from a pool:
  `(structure identity, view role, key within the view)`. The structure identity
  already exists and is already stable - it is what `clausters_history_register`
  mints and what `Editing.identity` caches - so no new notion of identity is
  introduced.
- `View.register` gains its inverse: a view can ask "what id draws this?" and get
  the same answer across redraws. The `widgets` map stops being rebuilt from
  scratch as the source of truth.
- The derivation lives in the shared crate from the start (a pure function over
  the key), so the two clients cannot derive differently. This is the one piece
  of Rust AP1 adds.
- `GuiIdAllocator` stays for hand-built GuiDefs, which have no structure behind
  them, and its docstring stops describing the multitrack's churn.

**Acceptance:** a redraw of any editor leaves every widget id unchanged for
every widget that still exists; an edit-back that crosses a redraw lands on the
widget the hand touched; a test drives a gesture, forces a redraw before the
answer is routed, and the picture and the data agree.

### AP2 - A redraw is a diff

What AP1 makes possible and what stops the id churn at its source.

- An application computes the difference between the tree it last sent and the
  tree it would send now, and emits `/gui_set` for what changed plus definitions
  and frees for what appeared and went. A whole-tree redefine stays as the
  fallback and as what `open` does.
- Screen state therefore survives a redraw with no resync, because the widget
  was never freed.

**Acceptance:** editing one clip in a piece of many emits no definition and no
free; a scroll position and a selection survive a redraw of the window they are
in; the fallback path is still exercised by a test, since it is what `open`
uses.

### AP3 - Screen state belongs to the host, keyed by the derived key

- Selection, zoom/`view_x`/`view_y`, layer and the hand's position stop being
  per-`Editor` fields and become state the host holds against the key from AP1.
- `NOT_AN_EDIT` stays the boundary it already is; what changes is where the
  answer is kept, not which tags are edits.
- Two views of one structure therefore agree about the selection without either
  of them pushing it to the other (`adopt_selection` goes away as a mechanism).

**Acceptance:** two windows over one structure show one selection with no call
between them; a selection is still readable from a script as a typed value
(`Ox` O6); nothing about screen state reaches a history or a file.

### AP4 - A payload is by value or by reference

The generalization of the asymmetry that exists today between `points` (whose
edit math crosses the ABI by value through `clausters_domain_edit`) and
`samples` (where it cannot, so the domain answers nothing).

- The application core takes payloads through one seam that says which of the
  two a payload is, and never branches on the vocabulary.
- Native, a by-reference payload may be shared memory; in the browser it is
  whatever the transport can do. **The correctness of the design does not depend
  on which**, and a test proves the two paths produce the same document.
- `SamplesDomain` stops being shaped differently from the others.

**Acceptance:** an edit over samples and an edit over points travel the same
code path in the application core; a by-reference payload and a by-value payload
of the same edit produce byte-identical results; the browser build compiles with
no shared-memory path at all.

*Risk, recorded because it is the weakest generalization here: this seam has one
implementor per branch today. If AP4 cannot be written without inventing a
second one, it waits - a trait designed against a single implementor is designed
wrong, and this plan says so about its own milestone.*

### AP5 - The application core moves to Rust

Only now, and only what AP0-AP4 have already proven is common.

- Down: id derivation (already there from AP1), the diff, the acknowledgement
  protocol (`Echo`), the routing table, the undo/redo walk across registered
  editors, and the unit bridge (beats/seconds <-> timeline samples).
- Stays in each client: `Domain.project` - writing a payload onto the client's
  own objects, which is by definition the client's - and a `View.build` for a
  view a client defines.
- The catalogue views (waveform, `bpf`, pianoroll, the multitrack's lanes and
  clips) build through `tree.rs`, so the standalone host gets the same function
  from the same code rather than from a second implementation.

**Acceptance:** the same gestures applied from Python, from the web client and
from a standalone host produce the same document *and the same tree of widget
ids*; the `editing/` line count in both clients drops to the domains, the views
a client defines and the idiomatic surface; no numeric or timing rule is left
implemented twice.

### AP6 - `FormEditor` converges

It is ported to the seam, not ported to Rust as it stands.

- `FormEditor` becomes an application whose structure is a document; `FormEditing`
  keeps what is genuinely the tree's (the held document, the node index) and
  loses what AP0 took.
- The lanes/clips projection is a view built through AP5's path.
- The mapping rule (root aggregate -> lanes, members -> clips, a nested
  aggregate as its summary until expanded) is unchanged; expand/collapse becomes
  screen state under AP3.

**Acceptance:** `composer.py` and the web client's equivalent page do the same
things by the same calls in the same order, read side by side, verb by verb; the
whole loop still works - built in Python, drawn, edited by hand, heard, undone,
redone, saved, reopened.

### AP7 - A second application, to prove the abstraction

An application that is **not** the multitrack: a small bundle of editable
subviews over structures with nothing composed behind them - a buffer, a curve
and a timeline in one window set with one undo order. It is written as an
example (`clients/python/examples/`), in both clients, because an abstraction
with one real consumer has not been tested.

**Acceptance:** it is written with the supported surface and nothing is added to
the clients to make it possible; the two versions are one program in two
languages; an undo walks the interleaving of what was done in each subview.

### AP8 - The pass over the packages, and the plans keep what is worth keeping

The milestone that makes deleting this file legal.

- **Docs:** `docs/architecture.md` gains the application scope and its place in
  the four layers; `docs/gui-protocol.md` takes whatever the diff and the derived
  id change on the wire; the Python book's composition chapter and the web
  book's equivalent follow; `docs/bindings.md` and the parity tests take every
  new symbol. `scripts/check-docs.sh` before committing anything a book reads.
- **Decisions:** `docs/decisions.md` records the two worth recording - the
  application as the unit that owns the id space and the screen state, and a
  widget id derived from a structure's identity rather than leased.
- **The plans keep the durable half of this file:** the new track in
  `clients/gui/PLAN.md` (the seam, the derived id, the diff, the screen state),
  a pointer in `clients/python/PLAN.md` and `clients/web/PLAN.md`, and - in
  `crates/clausters-document/PLAN.md` - the answer this branch gives to the open
  question about the second document, or an honest statement of what it still
  does not settle.
- **Checks:** `cargo fmt`, `cargo clippy --all-targets` clean,
  `.claude/skills/feature-matrix/check.sh`, `npx pyright` in `clients/python`,
  `./build.sh && ./test.sh` in `clients/web` with the parity vectors regenerated,
  and the touched examples run by hand.

**Acceptance:** nothing in this file is the only copy of anything, and deleting
it loses no decision.

## The order, and why it is this one

1. **AP0 before anything**, because moving the seam in Python is cheap and
   moving it after AP5 means moving it in Rust and in TypeScript too.
2. **AP1 before AP2**, because a diff over leased ids is a diff over noise.
3. **AP1-AP4 before AP5**, and this is the load-bearing one: lowering the core
   before the seam exists freezes the wrong unit in Rust, with the id lease
   inside it.
4. **AP6 after AP5**, because converging `FormEditor` onto a seam that is still
   moving is converging twice.
5. **AP7 after AP6**, because the second consumer is only evidence if the first
   one is already on the abstraction.
6. **AP8 last**, and the open question about the application document is
   answered there or explicitly left open - never decided in passing by a
   milestone that only needed somewhere to put a file.

## What would make this branch wrong

Written down so it can be checked rather than felt:

- If AP4 needs a second implementor invented to exist, the seam is speculative
  and waits.
- If AP5's move requires the document to read an opaque leaf, the projection is
  in the wrong place and the summary callback is what is missing.
- If AP7 cannot be written without adding surface to a client, the abstraction
  is not general and AP0's boundary is drawn wrong - not the example.
- If either client ends AP6 with a verb the other lacks, the branch has produced
  the divergence it was opened to remove.

## Status

- [ ] AP0 - the seam, in Python only
- [ ] AP1 - a widget id is derived
- [ ] AP2 - a redraw is a diff
- [ ] AP3 - screen state to the host
- [ ] AP4 - by value or by reference
- [ ] AP5 - the application core moves to Rust
- [ ] AP6 - `FormEditor` converges
- [ ] AP7 - a second application
- [ ] AP8 - the pass over the packages
