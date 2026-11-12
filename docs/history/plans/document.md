# clausters-document - implementation plan: the closed tracks

The tracks of [`crates/clausters-document/PLAN.md`](../../../crates/clausters-document/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

## The milestones

- ✅ **O1 - The document: the tree, and a leaf is opaque.** The crate's types and their serde form: elements with their placements (`onset`, `duration`), aggregates with their two kinds (**concrete** - members relate in time; **logical** - they relate by processing), and a leaf as `(id, kind, opaque config)`. A **source reference** carries its **lifetime** (external / session / temporary) from the start rather than gaining it later, since it is what a save has to read. No client objects, no widget, no OSC, no I/O. **Two properties the shape has to admit, because they are the arrangement's and not the document's to invent.** A generator's *code* is the opaque leaf; **its output is ordinary tree** - a generator may produce any element, generators included - so nothing about being generated makes a subtree a second kind of thing. And a **clang may reference a generator** to fire it live, which means the document expresses structure resolved at run time and not only at render time: a reference that no flattening pass will ever expand. **The tree stays general, and the views carry their own restrictions** - a multitrack lane is a *projection* that may decline to show what its shape does not admit, exactly as an unknown widget is laid out and not painted; nothing here grows a lane, a vertical position or a type-per-container so that a view is easier to write. **[Withdrawn 2026-09-06 - see "The turn: the arrangement stops being a projection". A projection is not free of structure: the lane, the order within it and the placement's identity are durable and undoable, and with nowhere to live they fell into the widget tree, which is where every multitrack defect of the `application-scope` branch comes from. O1's types are not deleted - they become what a region may contain - but the arrangement stops being derived from a general tree.]** The arrangement's own vocabulary (the Aggregate and the other primitives, the temporal traits) is unchanged by this milestone and is refined by iteration in `clausters.form`, which is why the shape has to stay versatile rather than final: the document need not know the model, but it must not be what blocks its refinement. **Acceptance:** a tree round-trips through serde unchanged; an unknown body kind and an unread config blob both survive a load/save cycle **losslessly**, and writing is **deterministic** (the two are the properties that matter, and byte-identity is not one of them: `serde_json` sorts an object's keys, key order in JSON carries no information, and buying its preservation would mean turning `preserve_order` on for every crate in the workspace since features are additive); the Python `clausters.form` tree converts in and out with no loss.
- ✅ **O2 - The intent vocabulary, and the one applier.** The intent enum - absolute only - and `apply(document, intent) -> Outcome`, where the outcome carries the **effective** value and therefore *is* the acknowledgement's content. Every transformation the owner performs (snap, clamp, a refusal on read-only material) happens here and is reported here, so no caller can apply an intent by hand. **The vocabulary has no relative form and that is the whole of the rule here**; the one payload that violates it is the host's `"transpose" <xml:id> <steps>` on an engraved page (`crates/clausters-gui/src/host/elements/score.rs`), and converting *that* is host work rather than the crate's, so it rides with H1, which is already the milestone that passes over every payload the host emits. The absolute form is available to it: the host re-derives staff position from the engraving in order to draw ledger lines, so it can name the position a note reaches instead of the steps it moved. **Acceptance:** applying the same intent twice leaves the same document (idempotence); a refusal reports the unchanged value rather than an error; every outcome names an effective value, and a test enumerates the vocabulary so a new intent cannot be added without one.
- ✅ **O3 - The acknowledgement across the three legs.** The wire half of O2: the host stamps each intent with a `seq`, the owner answers with the state push plus `/gui_ack <seq> <doc_version> [<source> <generation>...] [<reason>]` in the same bundle, always - the versions ride from the first message, since O4 reads them and a second pass over four ends costs more than carrying the field early. Host side: the pending set keyed by `seq` and the one drop-and-adopt rule - the mechanism, not the picture; what a pending edit *looks like* on a given widget is that widget's milestone (the GUI track's D1 for a signal element). Python: `GuiHost` gains bundle sending (it has none - `set()` emits loose messages) and `ack()`; the TypeScript builder ports in the same commit; `docs/gui-protocol.md` gets the verb. **Acceptance:** the generator-note case in "Found by use" below draws the note back where it was, with no redefine and no second message; two gestures in flight resolve independently and in either order.
- ✅ **O4 - The version, and staleness.** The document carries a monotonic version and every source its own generation; an intent names both; the owner reports staleness instead of applying blind. This is what closes the case the log alone cannot see - the document moving under the host by a route that is not a gesture (a script editing the arrangement, a second editor, a `follow` re-render). **Its granularity is settled** (see the decisions above): the document's version and a per-source generation, with an intent naming both. **Acceptance:** an intent made against a superseded version is reported as stale and the host re-syncs rather than losing the edit silently.

  **Staleness is detected, never rebased, and that is the decision the milestone had to take.** An absolute intent needs no rebase - that is what makes it absolute - but absolute and *safe* are not the same thing: `SetMembers` states an aggregate's contents **whole**, so one made against an older picture silently deletes whatever arrived in between, which is the exact failure this whole mechanism exists to make impossible. So `apply` takes an `Against` (the version, plus optionally the generation of the material) and refuses a superseded edit, handing back the value that holds. Merging the two instead was rejected: deciding which edit wins per field is a document format's decision, not an edit vocabulary's, and getting it wrong loses work quietly. Refusing costs a redone gesture; merging costs a lost one. Its refinement - a per-node revision, so an edit is stale only against the node it names rather than against the whole document - is under "Future directions": it matters when two people edit at once, and nothing does yet.

  **Nothing new on the answer path, one integer on the ask path.** The acknowledgement already carried `docVersion` (O3 shipped it early for exactly this), so what O4 added is the other direction: `/gui_event` grew a `version` third argument - what the host had last been told - read at send time from the outbox rather than carried in the gesture's effect, because it is the **conversation's** state and not the gesture's. On the owner's side there is still no branch: a stale edit is answered the way a snapped one is, with the value that holds, and the optional `reason` is what separates *someone else changed this* from *not here*.

  **Zero is reserved on both counters, and version now starts at one.** A document at version zero was both a real state and the sentinel for *unstated*, which made a fresh document's first edit unnameable - found by the acceptance test itself. `Document::new` starts at `FIRST_VERSION`, mirroring the host's stamps, and Python's `to_document` defaults the same way. Unstated still applies unchecked, which is what a script that just read the document wants and what an older host looks like.

  **Two gaps closed on the way, both found by writing the owner's side.** A **redefine** now drops that window's pending edits (`/gui_def` replaces the whole tree, so an edit in flight has nothing to resolve to and its id may already belong to something else - the same reasoning `/gui_free` already had, and the pending set would otherwise stay open forever). And **opening announces the version** with a stamp of zero, which retires nothing and carries only the number - without it the host names zero until the first acknowledgement returns, making the opening gesture the one edit nobody could tell was stale.
- ✅ **O5 - The log: undo lives with the document.** The pure log - entries, the gesture as the transaction, the label, the cursor, the prune on free, coalescing, the budget - plus the **spill store** behind a trait (a temporary directory natively, memory in the browser) for an inverse whose content is data, content-addressed so an undo/redo pair holds one copy. **A deterministic operation stores its parameters and recomputes the redo rather than storing the span**, which is only possible now that the log sits with the document: the owner has the algorithm, and the host - which was going to hold this log - never did. All of it **in the crate, beside the data it inverts**, which is the placement the H track had wrong and the reason its premise kept fighting itself. What the host keeps is what it always had: it emits the intent carrying the previous value, and it draws. The inverse of an entry is an ordinary intent, so undo needs no second path: it is O2's `apply` again. **Acceptance:** a scripted run of gestures inverts back to the starting document exactly; an inverse re-emitted after an undo is byte-identical to the intent the gesture first sent; what arrives from the owner after an intent is applied never enters the log.

  **The asymmetry is the whole design, and it only exists because of where the log sits.** Going **back** is always data - undoing a normalize is the old samples, and no algorithm reconstructs them - while going **forward** need not be, since a deterministic operation stores its parameters and is re-run. So the two directions are not the same type: `undo` hands back plain `Intent`s, `redo` hands back `Step`s, one of which the caller re-runs itself. A log held by the host could not have this arm at all, which is the concrete form of the placement argument rather than a restatement of it.

  **The inverse is read out of the document, not built by the caller.** O4's `current` - written for the staleness check - turns out to be exactly the inverse of an absolute intent, so `apply_logged` reads it before the edit lands and records the pair. That makes the rule mechanical instead of a habit: nothing is recorded unless the document changed, a refusal leaves no entry, and what an owner pushes back after a gesture cannot enter the log because it never goes through the door. `WriteSamples` is the one edit this cannot do alone - the samples it overwrote are not in the document - so a destructive caller reads the span, applies, and records the pair itself.

  **Coalescing is the caller's call, and the merge keeps the oldest inverse.** The crate has no clock and cannot know where a hand stopped, so an entry says whether it *continues* the one before it and the log merges when both touch the same node the same way. Keeping the oldest inverse with the newest forward is what makes one undo of a hundred small adjustments land where the run started rather than one step back into it.

  **What the spill store is, and what it is not.** A trait plus `MemorySpill`; the file-backed one lands with the first caller that needs it rather than now, because the crate has no business choosing a temp-directory policy and there is no native caller yet. Content addressing is not an optimization here but the ordinary case: a stroke writing silence over silence names the same span on both sides.

  **Not done, and deliberately: a per-node revision.** Coalescing, the budget and the cursor are all in; what is under "Future directions" is refining staleness from per-document to per-node, which the log makes affordable and which nothing can observe until more than one hand edits at once.
- ✅ **O6 - The typed selection.** A `Selection` as a value rather than a highlight: a **time span**, optionally restricted by a **value rect** on an axis that measures something, optionally a **spectral region** (frames x bins), optionally a **mask**. It is a document-level value a script can read back, hand to an algorithm and paste - which is what the brief asked for and what a pair of floats on the navigation group cannot be. Its round trip obeys K5. **Acceptance:** a selection survives a query/set round trip in every variant; the two-number form scripts already read keeps working.

  **It is a value and it is not in the tree**, which the four layers had already settled without the milestone noticing: a selection in flight is *screen state*, never persisted and never logged. What O6 adds is that the same selection can be **read out** - crossed over a wire, kept in a script's variable, handed to an operation - so `Selection` is a type the crate defines and not a field of `Document`. That also settles where its serialization belongs: it is a value crossing, so how it is framed is the caller's.

  **One time span first, and the axes that narrow it.** That is what makes an arrangement selection, a sample selection and a spectral selection the same kind of thing. `value` (the signal's own units) and `bins` stayed **separate fields** rather than one "second axis": they mean different things and are read by different code, and an operation that understands one need not understand the other.

  **The unit is whatever the selected thing is measured in** - frames over material, beats over an arrangement - and the crate does not convert, because the beats↔samples bridge belongs to whoever renders. Both travel as `f64`, which holds a frame index exactly past any length a session will have and is what the wire already sends. A tagged unit was considered and dropped: it would have broken the plain two-number form for nothing the caller does not already know.

  **The lasso is a mask and not a polygon**, per D6, and the reason is worth keeping: every reader of a region wants to ask *is this cell in* rather than re-rasterize an outline, and an intersection of two regions then needs no geometry at all. It reads out of range as *out* rather than panicking, because it arrives over a wire.

  **The compatibility half is structural rather than a shim.** Every narrowing field is omitted when absent, so a selection that is only a span serializes as exactly the two numbers the `"selection"` payload has always carried - which is both the acceptance and K5. `Selection::is_plain` is what a reader checks before taking the short path, since treating a spectral region as the whole band is the quiet kind of wrong.

  **Not here, and on purpose:** resolving a selection to a source range is O9, and the wire and client legs arrive with the bindings (O10) - nothing outside the crate consumes a `Selection` yet, and shipping a builder with no caller is a declaration rather than a feature.
- ✅ **O7 - The typed clipboard.** One document with a `kind` - notes, samples, a spectral region - so one mechanism carries every payload across windows and defs. K6's host-wide `String` gives way to it (the decision is above): the structure is JSON, the bulk rides beside it as little-endian `f32` blobs, and the notes block that travels as a string today keeps working because a string is one of the kinds. **Acceptance:** a sample block copied in one window pastes in another; a block carries its sample rate and is **not** resampled in transit - resampling is an edit the owner performs, never a side effect of a paste.

  **The structure names blobs; it does not hold them.** `Clipboard` serializes whole as JSON and a bulk payload appears in it as an **index** into the blobs travelling alongside - the same convention a GuiDef's `"blob": <index>` prop already uses - so one clipboard crosses a wire, a window boundary and a process with nothing re-encoded on the way. `blobs()` says how many must accompany it, which is what lets a receiver tell a **truncated** paste from an empty one; pasting silence would be worse than declining.

  **The rate is carried and the crate has no conversion to run**, which is a stronger statement than "does not resample": resampling is an *edit*, something an owner performs and logs, so a paste reads `sample_rate()` and decides. `values()` validates a payload against its header, since a header and a blob arriving separately is exactly where a mismatch hides.

  **Four kinds, and one of them is the tree.** Text (what the clipboard was before it had kinds, and how the flat notes block still travels), **elements** - placed `Member`s, so a copied selection keeps its relative offsets and the recursion is the tree's own rather than a second shape - samples, and a spectral region. A spectral region carries `values_per_bin`, which is one integer and covers both cases D7 needs: 1 for magnitudes when a region only has to be measured, 2 for interleaved real/imaginary when it has to be resynthesized with its phase.

  **The compatibility is a door, not an untagged guess.** `Clipboard::parse` reads a clipboard document if it is one and text otherwise. An untagged fallback would read a *stored string that happens to be JSON* as a structure, and paste a document where the person copied a line - the silent kind of wrong. The little-endian `f32` conversion lives here for the ordinary reason: three languages writing the same byte order three times is three places for it to be wrong, and the wrong one sounds like noise rather than failing.

  **Not here:** the host's own clipboard is still a `String` and the client legs arrive with the bindings (O10). Nothing outside the crate holds a typed clipboard yet, and the K6 replacement is a GUI-track change (D4) against this type.
- ✅ **O8 - The session document** *(C38 relocated from `clients/python/PLAN.md`, where it was opened 2026-08-13 alongside the H track; the entry there becomes a pointer)*. The document written to a file: concrete material by path and range, placements, rendered event scores, control curves, plus **configuration as the opaque blob O1 already carries** and **provenance** - the reference to the scripts that generated it, which is what makes re-generating possible without the document knowing how. It lands here rather than in a client because the `standalone` host is the other writer of the same format, and a format with two writers in two languages is a format that drifts. It must also express a generator leaf's **last rendered result**, since that is what a host with no language shows, and **an open edit**: a promoted scratch, its provenance, and the fact that confirmation has not happened, since a save never blocks on one. **Acceptance:** a session written by the Python client opens in a standalone host and vice versa; a generator element's blob survives both directions unread; a session saved mid-edit reopens with the edit still open.

  **What "an open edit" means, narrowed 2026-08-17** by the decision *"Does the working copy still lead, now that a write costs the span?"* (Open decisions, taken): a take **loaded** into a server buffer is already a copy of the user's file, so it is edited where it lies and there is no confirmation step — the acknowledgement and the log entry are what settle a stroke. `OpenEdit` and `Lifetime::Temporary` keep the job that is genuinely theirs: material reached **by reference** to the user's own file, where an edit must materialize a copy before it writes something the four-layer rule forbids writing. The format is unchanged; what changed is when a writer is expected to use this part of it.

  **The hole the milestone found is that a generator's last rendered result was not expressible at all.** `Body::Generator` held a configuration and nothing else, so the one thing a host with no language attached can actually *show* had nowhere to live. It is now a field on the generator node - ordinary tree hanging off an opaque leaf - and it is part of the **format** rather than a cache, because a cache can be missing and then there is nothing to draw. It draws one line worth keeping: a **reader** walks into it (`walk`, `find`, `max_id`, so an id inside a rendering is reachable and a client continuing to allocate cannot collide with one) and an **intent** does not, because a rendering is not the composition and editing one writes over what the next render replaces.

  **A session is the document plus the half it deliberately lacks.** The tree says *what plays when* and not where a source lives, because inside a running system a source is a server buffer, a mapped file or a rendered result and the tree has no business knowing which. `Session` adds the table, and two of its fields are the ones a naive format leaves out and then cannot add: **provenance**, carried opaquely, which is what makes re-generating possible without the format knowing how; and an **open edit**, which is what lets a save mid-edit be honest rather than either silently confirming or refusing.

  **Three things a save and an open have to report rather than discover.** `volatile()` names material never written down - saving is not blocked by it, since blocking the safest habit in the program is the wrong trade, but the file cannot claim to be complete either. `dangling()` names sources the tree references and the table lacks, up front instead of one element at a time halfway through drawing. And `is_readable()` refuses a newer *format* rather than half-reading it, while a newer *field* is not a version change at all - it is ignored on the way through, the way an unknown body is carried rather than dropped.

  **The acceptance crosses for real.** `session_vector.json` is written by the Python client (`to_session`, beside the existing `to_document`) and parsed by `tests/form_parity.rs`, so the source table, the promoted-but-unconfirmed scratch, the volatile source and the generator's frozen result all prove the two writers agree. The **standalone host** is the third writer and is not one yet: it takes the format with O10, and until then this is the crossing that exists rather than the one the acceptance describes in full.
- ✅ **O9 - A selection resolves to a source range.** The half of the old D5 that is not a client's: mapping a selection back onto the element it came from - a buffer range, a file span, a rendered element's samples - through a clip's trim and the beats-to-samples bridge, so the ordinary editing operations can be applied to it *by whoever owns them*. The operations themselves stay where they are: numeric work that anything outside a window would call belongs to `clausters-core`, and a user-written function belongs to the user. **Acceptance:** a selection made on a clip's body resolves to the right span of the take underneath it, trim and offset included.

  **The tempo is the caller's; the arithmetic is here.** `Mapping` takes **frames per beat** rather than a tempo and a sample rate, which keeps the crate out of a policy it has no business in - the beats↔samples bridge belongs to whoever renders - while still doing the conversion once instead of in every client. Same line the crate draws around a leaf's configuration: carry what is given, own what is shared. The **unit** is a parameter of the resolve rather than a field of `Selection`, for the reason O6 already recorded: tagging the value would have broken the plain two-number form for nothing the reader does not already know.

  **Three terms, and getting any of them wrong is silent.** The placement's base accumulated through the tree (a nested group ten beats in), the trim (which part of the source this element uses), and the clamp at both ends - a selection dragged past a clip resolves to what the clip covers and never past the end of a file. The test that guards it places a clip at beat 2 reading its take from beat 10, so no arrangement of the terms passes by accident.

  **Two things the answer carries that a caller would otherwise look up afterwards.** The **generation**, because an operation reads material and a read taken against an older one is exactly the case the two counters exist for; and `at`, where each piece sits inside the selection - without it a selection crossing three takes resolves to a bag of spans with no way to reassemble them, which is a copy that cannot be pasted.

  **What has no span to give is skipped rather than reported**: a group and a generator are in the way of a selection, not underneath it, and the caller asked what is underneath. A placement with neither a length nor a trim gives nothing rather than guessing "the whole file", which would be an operation reading material the composition never used.
- ✅ **O10 - The bindings and the books.** Python binds the crate and `clausters.form` becomes accessors over its tree rather than a parallel model; the web client binds the same one; the GUI host takes the intent vocabulary **and nothing else**. Docs: `docs/architecture.md` gains the four layers and this crate's place in them; `docs/decisions.md` records the two calls worth recording (the single document and the immutable sources; the acknowledgement as a stamped state push rather than a reply code); `docs/bindings.md` and the parity tests take whatever crosses an ABI. **Acceptance:** the same composition edited from Python, from the web client and from a standalone host produces the same document.

  **The binding is one function, and that is the decision rather than an economy.** Every other stateful surface in `clausters-ffi` hands out an opaque handle; this one does not. A handle would mean each client holding pointers into a Rust object graph, and then every accessor a client wants - and a tree has dozens - becomes a call to design, bind and keep in step. Round-tripping the format costs a serialization per edit and buys the property the plan's decisions already required: a client's document **is** the crate's document rather than a parallel structure that synchronizes with it. `clausters_document_apply` and `clausters_document_resolve` (`CORE_ABI_VERSION` 15), their wasm peers, their rows in `docs/bindings.md`, and their ctypes and TypeScript faces.

  **The acceptance is a test rather than a claim.** `clients/web/tests/document-vectors.json` is a composition the Python client builds and edits **through the C ABI**, frozen after each of nine edits - a snap, a whole-configuration replace, an unnameable node, a stale version, an unstated one, a destructive write and its idempotent repeat - plus four selection resolutions. `document-parity.test.ts` runs the identical edits through the wasm door and compares documents and outcomes byte for byte. Nothing else would catch a divergence: cargo checks each binding against the crate and never against the other, and no build reaches either client's call sites, so a snap implemented twice or a version bumped on the wrong side would ship green. Writing the vector immediately found two of its own cases naming the wrong node, which is the argument for it.

  **The GUI host did not take the vocabulary, and that is the honest report.** The milestone says "the intent vocabulary **and nothing else**", and today the host constructs no `Intent`: it emits `/gui_event` payloads in the owner's terms and draws what comes back, which is the whole edit-back design. Adding the dependency now would be a declaration with nothing calling it - the same call K5 made about `Element::event()`. The forcing case is a **standalone host that edits**, which is the GUI track's H3, and the dependency lands with it. *(It landed: H3 closed 2026-08-16, and `crates/clausters-gui/src/host/document.rs` builds `Place` and `WriteSamples` from `/gui_event` payloads — the prediction held, including that the host would take the vocabulary and nothing else.)*

  **Docs.** `docs/architecture.md` gained the four layers and the crate's place under "The arrangement layer" (with O5); `docs/decisions.md` gained the two entries the milestone names - the single document with immutable sources, including why MVC stops working once a view edits, and the acknowledgement as a stamped state push rather than a reply code; `docs/bindings.md` gained the document section with the no-handles reasoning; and the Python book's composition chapter gained "The document: what the composition *is*, and who edits it" - the conversion, `document_apply`, and sessions.

  **Not done, and named rather than implied:** `clausters.form` is still this client's own object model with a conversion to the document, not accessors *over* the crate's tree. The conversion is lossless and id-stable and the crate is normative through it, which is what the decisions asked for ("the clients round-trip; they do not hold handles"); turning the Python objects into views over a shared tree is a further refactor with no consumer waiting on it. *(It has one now, and it is cost rather than ergonomics: **O12** measures what "a serialization per edit" costs on a real composition and is where that refactor is decided.)* The **log** (O5) is also unbound - it is stateful, and whether it crosses by handle or by value is a real trade rather than a detail - so that is **O11** below rather than a note here, because the GUI track's H2 depends on it.


  *(Revised 2026-10-03 by the user, and annulled: **the rule "the clients round-trip; they do not hold handles" no longer applies.** It was written before the applications that use the document and its history, for a design in which a client held the document and the crate applied edits to it. The applications (`clausters-apps`: the editors, the editing context and its history, the playback) moved that logic into Rust and hold the structures they edit, which also makes the standalone host the same program; a client holding one of those structures holds a handle -- the `EventSequence` since `X3.2` and `C57`, the multitrack since `C60` (`clients/python/PLAN.md`). What still crosses by value is data, not a rule. `docs/decisions.md`, "The round-trip rule is annulled: the applications hold the structures", says why.)*
- ✅ **O11 - The log crosses the ABI.** O10 bound the document and **not** the log, which leaves the GUI track's H2 (undo and redo from the hand, driven by the Python `Editor`) depending on something that does not exist: H3 is Rust and links the crate directly, but a language client cannot reach the log at all today. The design question is real and is **not** pre-decided here, because the two answers trade against each other. A **handle** (`clausters_log_*`, the shape `clausters_registry_*` already uses) keeps the spill store where it belongs - a log's whole point is that a bulk inverse leaves the log, and a by-value log would carry the spilled bytes back and forth on every call, which is the cost spilling exists to avoid. A **by-value** log would match the document's own binding and keep one rule for the crate's surface, at the price of either giving up spilling across the ABI or making the store the caller's to hold. Whichever wins, two things are fixed already: the inverse of an entry is an ordinary intent, so undoing needs no second door beyond the one O10 opened; and `Log::redo` hands back a `Step`, so a binding has to express "the owner re-runs this" and not only "apply this". **Acceptance:** a run of gestures applied from Python inverts back to the starting document exactly, through the crate's log rather than through one the client keeps.

  **The handle won, and the deciding term was the spill store.** A by-value log would carry every spilled span on every call - which is exactly the cost spilling exists to avoid, and at the default budget with sample inverses that is megabytes per undo. So the log stays in Rust with its store and the caller holds a pointer (`clausters_log_*`, the shape `clausters_registry_*` already uses; a `Drop`-backed class in wasm; a context-manager `Log` in Python). The document goes on crossing by value, and `docs/bindings.md` says why the two differ rather than leaving it to be inferred.

  **Applying and recording are one call, and that is a correctness rule rather than an ergonomic one.** The inverse has to be read out of the document *before* the edit lands, so a surface that let a caller apply first and record second would let it record the wrong thing. `inverse_of` became public in the process - a caller building its own entry needs exactly it, and it is also what a host's "every intent carries its previous value" (H1) is on the owner's side.

  **The bug worth keeping: size-then-fill over a surface that mutates.** The crate's JSON convention sizes with a null buffer and fills with a second call, which needs the payload identical on both - and everything here mutates. The first implementation undid on the *sizing* call and reported "nothing to undo" on the fill, and recorded two entries per edit. It is the same hazard `notation` records from the other side (why there is no one-shot engrave), met going the other way. The rule that resolves it: **the mutation happens only when the bytes are actually written**, so a sizing pass is free of consequence and a run of them is idempotent - which is what a binding has to be able to assume. `Log` grew `peek_undo`/`peek_redo` and `step_back`/`step_forward` for it, with `undo`/`redo` as the two together; two tests pin the invariant, including a buffer too small being a size query rather than a half-done edit.

  **The acceptance crosses all three sides.** `document-vectors.json` grew a `logged` section: four edits driven through the log from Python **through the C ABI**, then undone to the start and redone to the end, with the entry count and the menu label frozen at each step. `document-parity.test.ts` drives the identical run through the wasm `Log` and compares the documents - not the log, which is an object on both sides, but the states it produces.

  **What is still deferred, and it is O5's decision unchanged:** the spill store reachable from a binding is **memory**. A file-backed one is a `Spill` implementation a Rust caller supplies, and it lands with the first caller that needs it - which is the destructive-editing work in the GUI track's D1/D2, not a milestone here.

- ✅ **O12 - An edit costs the edit, not the document.** O10 bound the document **by value** and said why: a handle would mean each client holding pointers into a Rust object graph, with an accessor per field of a tree to design and keep in step, and the round trip buys the property the decisions required - a client's document *is* the crate's document. That reasoning stands. What O10 did not do is put a number on "a serialization per edit", and the number is the reason this milestone exists rather than staying a note under O10.

  **The measurement, on the composition the milestone is for** *(taken 2026-08-14, release build, one `Place` intent - the ordinary clip drag)*. A document of 10240 events (8 lanes x 40 clips x 32 notes) serializes to 3.3 MB, and one `document_apply` over it costs **205 ms**. The same call over the 320-event composition an example builds costs **6.0 ms**. The cost is linear in the whole document and independent of the edit, which is the shape of the problem rather than a constant to shave: a `WriteSamples` touching fifty samples costs the same 205 ms as a drag, and D1's stroke would pay it per stroke.

  Where it goes, so the milestone optimizes the term that is actually large:

  | Term | Cost at 3.3 MB | |
  |---|---|---|
  | the crate's **sizing** pass - parse the document, apply, serialize, report the length, discard | 79 ms | **pure waste**: the same work is done again immediately |
  | the crate's **fill** pass - the identical work, this time keeping the bytes | 79 ms | irreducible while the document crosses whole |
  | `json.dumps` of the document on the way in | 19 ms | irreducible while the document crosses whole |
  | `json.loads` of the reply | 24 ms | irreducible while the document crosses whole |

  *(A fifth term, 52 ms, was `bytes(out[:n])` building a tuple of n Python ints to slice a ctypes array; `ctypes.string_at` memcpys instead, and that is fixed and committed separately - it was every JSON-returning binding's cost, not the document's.)*

  **The sizing pass is half the crate's cost and it buys nothing here**, which is worth separating from the binding-shape question because it can be settled either way: `size_then_fill` exists so a caller can allocate exactly, and for a reply whose size is a function of a *mutation the crate just performed* it means performing the mutation twice. O11 already had to make that safe (the mutation happens only when the bytes are written); making it *cheap* is the other half, and the shape is an ordinary one - a caller-grown buffer retried on overflow, or the parsed document held across the pair. Whichever, it is measured before and after.

  **The anticipated shape is a document handle, and the tension with O10's rule has to be written rather than deduced.** "The clients round-trip; they do not hold handles" was decided against *accessor* handles - a pointer into the tree with a call per field - and the objection was surface area, not pointers. A handle that holds the parsed document and still exposes exactly `apply`/`resolve`/`snapshot` has the same surface as today's and none of that surface area, while making an edit O(edit): nothing is parsed, nothing is serialized, and only the outcome crosses. O11 set the precedent one milestone ago for the identical reason (carrying the payload whole on every call is the cost the design exists to avoid), and the log's handle is where a document handle would naturally sit anyway, since `clausters_log_apply` already takes the document on every call. The milestone still has to *decide* it, because two things push back and both are real: a handle makes the document stateful across the ABI, so a client can hold one that has drifted from what it thinks it holds - which is exactly the staleness O4 guards, now with a second copy to guard - and `snapshot` has to stay cheap or the property "the client's document is the crate's document" becomes a serialization by another name. Both are answerable; neither is answered here.

  **Three use cases force this, and they are the milestones rather than a schedule.** **D4 (paste)** creates nodes on the crate's side, so with two trees the client rebuilds objects and a script holding `my_clip` in a variable is left with an orphan - `_doc_id` makes reconciling possible and the reconciler is work that exists only because there are two trees. **D1/D2 (destructive editing)** pays the full document cost per stroke, which is the case where 205 ms is not a slow gesture but an unusable one. And **a script editing beside an open editor** - O4's "the document moved by a route that is not a gesture" - slips through today, because mutating a Python object bumps no version and the staleness check never fires; one tree is what closes it.

  **What must not change, and it is the crate's whole discipline:** the crate stays the only thing that applies an intent, and one tree rather than two remains the goal rather than a cost to pay for. This milestone is about *where the tree is kept and what a call carries*, not about what an edit means.

  **Acceptance:** a `Place` and a `WriteSamples` over the 10240-event composition each cost within a small constant of the same edit over the 320-event one - the figures above are the baseline they are measured against, and the number goes in the commit rather than in a claim; `clausters.form`'s public surface is unchanged for a script that never touches the document; the parity vectors of O10 and O11 pass unmodified, since nothing about what an edit *means* moved.

  **Done 2026-08-14. The handle won, and the measurement is the whole report:**

  | | before | after |
  |---|---|---|
  | one `Place` over 10240 events (3.3 MB) | 205 ms | **0.008 ms** |
  | the same over 320 events (0.11 MB) | 6.0 ms | **0.009 ms** |

  The cost is now **independent of the composition** rather than linear in it, which is the acceptance as written and is a stronger result than the "small constant" it asked for. Two separate things got it there, and the second is the one that mattered.

  **The handle removed the JSON.** `clausters_document_open`/`_free`/`_apply`/`_resolve`/`_snapshot`/`_version`, their wasm peers as a `Document` class, their ctypes and TypeScript faces, and `clausters_log_apply`/`_undo`/`_redo` taking the document handle instead of its bytes. That alone took 205 ms to 13.9 ms. **The objection O10 raised does not apply and the plan said so in advance**: it was against an *accessor* handle, a call per field of a tree, and this is the same three verbs the by-value binding had. The property it was protecting - a client's document **is** the crate's document - is now stricter, since there is only one copy. The by-value form survives in each client's own language (`document_apply` in Python, `applyIntent` in TS) built out of open → apply → snapshot → free, so a script that has a document in hand still has its one-liner and pays the serialization only where it asked for it.

  **The `clone` was the rest of it, and finding that is why the number is in the plan.** With the JSON gone, `apply` still copied the *tree* so that a size-then-fill sizing pass could leave the document alone - 13.9 ms on 10240 events, still O(document) and still independent of what the edit touched. The fix is that **an edit runs in place and is rolled back by its own inverse** (`inverse_of`, the one the log already records) with the version restored by hand, since applying an inverse bumps the counter rather than rewinding it. A `WriteSamples` has no inverse in the document, so that one path still copies - the rare case, and paying there keeps the rule exact everywhere.

  **The two size-then-fill rules, now stated rather than assumed.** A **mutating** call commits only when the bytes are written (O11's rule, unchanged, and now enforced by rollback rather than by copying); a **pure read** caches between the pair, so `snapshot` serializes the composition once instead of twice. Caching a mutating call the same way was rejected: the mutation would land on the sizing pass, and a caller that sized and gave up would have edited without knowing. Four tests pin it, including that a rolled-back sizing pass leaves the **version** alone - without that the document sits a version ahead of itself and every later edit reads as stale.

  **What did not move:** the parity vectors are semantically identical to O11's, key order aside (which O1 already recorded as carrying no information), and `document-parity.test.ts` passes over the regenerated file. Nothing about what an edit *means* changed, which is the point - this milestone is about where the tree is kept and what a call carries.

  **Still open, and it is the other half of the note under O10:** `clausters.form` is a Python object model with a conversion, not accessors over the crate's tree. O12 was the cost argument for closing that, and the cost is gone - so what remains is the *identity* argument (a paste creating nodes the client has no object for, D4) and the *two writers* one (a script editing beside an open editor, which bumps no version). Those are D4's and H2's to force, and they are cheap now that an edit is free.

- ✅ **O13 - One document, held: the editor stops re-deriving what it already has** *(opened 2026-08-17, the milestone number O12's closing paragraph and three "found by use" entries have all been waiting for; the roadmap named writing it as the step before starting it)*.

  **The measurement, because it is the whole argument.** O12 took one `Place` on a 10240-event composition from 205 ms to 0.008 ms by keeping the tree in the crate behind a handle. The bridge hands it straight back: `Editor._history` calls `to_document(self.element)` and opens a **fresh** `Document` on **every gesture**, so what a drag costs today is

  ```
                to_document   Document()   apply    per gesture
    320 events      1.18 ms     16.95 ms   0.013       18.15 ms
   3200 events     11.81 ms     23.42 ms   0.015       35.24 ms
  10240 events     35.96 ms     71.41 ms   0.014      107.38 ms
  ```

  The crate's own edit is 0.014 ms of 107. Everything else is the client re-deriving a document it had a moment ago, and it is O(composition) per gesture — the exact cost O12 exists to have removed.

  **What it is.** The editor opens **one** `Document` when it draws a composition (or loads one) and holds it for that composition's life; every edit applies to that handle and the arrangement is **projected from the outcome**, never the other way round. `_project` already does this for placements; what changes is that nothing rebuilds the document behind it.

  **Three things it forces, and they are why this milestone and "undo works for clips and for nothing inside one" (`clients/python/PLAN.md`) are one piece of work.** Once the document is held, an edit that writes the arrangement *directly* leaves it behind — so the two that do must stop:

  - **A note edit becomes a `SetMembers`.** The intent exists and says exactly this ("the roll's edit: notes added, moved and removed arrive as the resulting list. Members keep their ids"). What it needs is the ids, and they now exist: a timeline item is stamped with its node id on the way out and restored on the way back, so a note is addressable across a conversion and across a save.
  - **A break-point edit becomes a `Configure`.** That needs the curve's points **in the document**, which they are not today: an automation leaf carries a reference and nothing else, so there is no previous value to invert. A leaf's config is opaque and this is what it is for.
  - **A script that edits the arrangement behind the editor's back needs a door**, since with a held document it is no longer silently absorbed by the next rebuild. That door is explicit (`Editor.refresh()`), and it is also what finally makes O4's staleness detection do something: today mutating a Python object bumps no version, so the check never fires.

  **What must not change**: `clausters.form`'s public surface for a script that never touches the document; the crate stays the only thing that applies an intent; and the arrangement objects stay ordinary Python objects a script can hold in a variable. This milestone is about **where the authority is and how often it is rebuilt**, not about turning the client into a set of accessors — that maximal reading is a *later* question, and it is not what the cost or the correctness argument asks for.

  **What it does not take on**: a paste creating nodes the client has no object for (D4's identity question) and *"May one element be placed twice"* both become tractable here and are decided on their own, not by accident inside this.

  **Acceptance:** a gesture on the 10240-event composition costs the edit rather than the composition, measured against the table above; a note dragged in a roll and a break-point dragged on a curve are each undoable and redoable, by test and by hand in `composer.py`; a script that mutates the arrangement while an editor is open either sees its change adopted through `refresh()` or has its next gesture refused as stale, rather than silently winning; and `clausters.form`'s surface is unchanged for a script that never opens a document.

  **Done 2026-08-17. The measurement is the report:**

  ```
                before     after
    320 events   18.15 ms   0.019 ms
   3200 events   35.24 ms   0.020 ms
  10240 events  107.38 ms   0.020 ms
  ```

  Flat in the composition, which is the acceptance as written and is the same result O12 got one layer down.

  **What it took, and one of the three was not in the plan.** The document is opened once and held (`_rederive` is what says it must be derived again). A note edit is a `SetMembers` whose members keep their ids **positionally** — the roll sends the resulting list and order is the only information there is — with anything extra minted past the arrangement's maximum (`next_node_id`, the conversion's own rule, so a minted id and a converted one cannot collide). A break-point edit is a `Configure` over a config that now **carries the points** (`_points_of`), which also fixed something nobody had filed: an edited curve did not survive a save at all, because reopening resolved the automation by name and took whatever envelope that object happened to hold.

  **The third thing was the redo path, and removing it is the milestone's real prize.** A redo *adopted the whole document* and walked it against the client's objects — O(document) per step, a second implementation of what an edit means, and the asymmetry behind "a redo moved the model and told the host to keep drawing the old position". The log's redo now **reports the intents it applied** (`redone`, beside `remaining`) in both faces, so a redo is the same shape as an undo and takes the same path: `_project`, once per intent. `Editor._adopt` is deleted rather than extended, which is the opposite of what the reverted first attempt at this had to do.

  **What is deliberately still direct**: a patch cord (`_apply_wire`) rewrites two members' controls, which no intent describes yet; it sets `_rederive` and is undoable by nothing, exactly as before. That is the next thing this route wants, and it is not this milestone's.

- ✅ **O14 - A node id names a placement, and what may be placed twice is what a node *references*** *(opened 2026-08-17; the shape half of the open decision "May one element be placed twice", whose *choice* the user settled on the same day. O13 is what made it tractable: one held document, and ids under control.)*

  **The choice, restated in one line so the shape can be read against it:** a clip is a **window onto material** and the identity is the material, so *forbid* and *copy* are both wrong and the intent must name the **placement**.

  **The shape, and the surprise in it: the format already does this.** A `Place` names a node, and a node is what a `Member` holds — so the document has been able to express two windows all along. What collapsed them is the **bridge**: it stamps the node id on the *element object*, so one Python object placed at two offsets writes two members carrying one number. Move the stamp to the **member handle** — which `Group.add` already returns and whose docstring already calls it "the stable identity `remove` and `move` take" — and each placement is its own node, addressed unambiguously, with no change to the crate, the wire, or the intent vocabulary.

  **What that leaves is one real question, and it is the instance/function axis:** two windows share material only when the material is something the node **references**. So the rule is drawn there:

  - **A leaf that references its material may be placed twice** — a buffer (two nodes, one `source`), a generator, a sequence naming a pattern. Two windows onto one take, two evaluations of one function: the multitrack's own semantics, and the case the user's exposition is about.
  - **An element whose material is *in* the node may not** — an event (its config), a track (its notes), a group (its members). Two nodes there are two **copies**, and they diverge on the first edit, which is the answer this decision rejected. It is refused with a message that teaches the distinction rather than silently copying.

  **What stays open after it, deliberately**: the **alias** — a node that says "my material is that node" — which is what would let a *container* be placed twice and edited in one place. It needs a body kind and a rule for what an intent naming an alias means, and nothing today asks for it. And an evaluation's **arguments** (a function placed twice with different parameters), which is the same door: a placement's own configuration, once a placement is a thing with an identity.

  **Acceptance:** `Group([(0, take), (4, take)])` over a leaf that references its material converts to two nodes with different ids, both naming one source; dragging the second moves the second, in the document and in the arrangement, which is the case the defect at the foot of this file was written about; the same over a track or a group is refused, naming why; and a session round-trips with each placement keeping its own id.

  **Done 2026-08-17, and the crate did not change.** The stamp moved to the member handle (`_Ids.of(element, member)`), the conversion walks handles rather than triples, and `from_document` puts each member's id back on the handle it built — so an element reached as a placement carries no id of its own, which is the rule stated where it can be read. `Editor._node_id` takes the placement when the caller has it and declines when an element is placed twice and nobody said which window, since there is no way to tell from an element alone.

  **The refusal is one function** (`_placeable_twice`) and it is where the instance/function line lives: a `Vector`, a `Generator` and a pattern-backed `Sequence` are windows onto material the node only *names*, so a second placement is allowed; a `Clang`, a `Track` and an `Aggregate` carry their material inside the node, so a second placement would be a second copy and is refused with the distinction rather than made in silence.

  **What it cost elsewhere: nothing.** No wire change, no format change, no crate change — which is the finding worth keeping, because the decision had been costed as *"the most expensive"* of the three answers for months. It was expensive against a model where a member has no identity; the member handle has been that identity since the day `Group.add` started returning one.

- ✅ **O15 - The stack belongs to the data: one registry per editing context** *(opened 2026-08-30 with the user, after measuring that two editors over one arrangement keep two histories — `clients/python/PLAN.md`, "Two views of one arrangement keep two histories, so an undo writes a state nobody was in"; **reshaped 2026-09-01 with the user**, when the requirement turned out to be wider than the arrangement: the stack has to serve a structure the client built on its own — a roll, a curve, a buffer with no composition behind it — and a `GuiDef` application that composes several editable subviews, whose history is the interleaving of what was done in each. What that moved is recorded below.)* O5 put the log beside the data it inverts and gave the reason: a log that sees only one editor's gestures describes a document that has moved on. The client then re-introduced exactly that one level up — `Editor` mints its own `_native.Log()` per instance while the arrangement is shared objects, and the web client does the same — so the placement argument has to be made *structural* rather than repeated: a log must be reachable **from the data**, not constructed by whoever draws it.

  **The rules this milestone rests on, settled with the user.** They are written here because they are the whole of the design and each one closes a question that would otherwise be answered by accident inside the work:

  - **A stack belongs to the data, never to a view.** Two editable views of one structure share one history, and an undo in either updates both.
  - **A stack is temporary.** It is runtime state and is never serialized — which the format already agrees with: `session.rs` carries the document and the source table and says nothing about a log. So this milestone changes no file format and owes no migration. It also means the registry is **not** a field of `Session`, which is the serialized type; it is a runtime structure a session holds.
  - **It lives as long as the data is resident, and dies when the data is deleted.** Closing a *view* is not an event of the history; closing the *data* is.
  - **Linear.** A new edit after an undo truncates the redo; branches are not kept.
  - **An entry may group several operations** (O17), because one gesture can touch more than one structure.
  - **What is never an entry:** the selection, the navigation window, the focus, the edit layer, the playhead. Screen state, as the four layers already say.

  **The shape: the registry is the scope.** A registry holds **one ordered pile with one cursor**, plus the map `structure identity -> the domain that state belongs to`. A caller registers the data it is about to edit and gets an identity back; every edit over any of those structures lands in the one pile, in the order it was made; undo walks that pile. So:

  - an **independent element** — a curve the client built, a buffer, a roll with no composition behind it — is a registry with one structure in it, and gets a history without a document existing anywhere;
  - a **combination** — a `GuiDef` application composing several editable subviews — is one registry with several structures in it, and the interleaved order its undo walks **is** the pile, with no second mechanism to produce it;
  - **two views over one arrangement** are two views over one registry, which is the inversion of the measured defect.

  What decides what shares a history is therefore *which registry a structure was registered in*, never which window is looking at it. **A structure belongs to exactly one registry**, and composing a view over structures registered in different ones is refused, naming why — that is the rule the whole track exists for, said once, in the only place that can enforce it.

  **What the reshape removed, and why it is worth naming.** The earlier shape was a map `structure -> Log` plus a **monotonic sequence counter owned by the session**, so that a view over several structures could merge its stacks into "the latest among these". With one pile the counter is the pile, and the merge is nothing. The alternative that keeps per-structure stacks was measured against this requirement and rejected for a concrete reason, not for size: a view that filters one shared order down to the structures *it* shows is doing **selective undo** — inverting an entry that touched A and B while a later entry over B stands — which writes a state nobody was in, the exact defect this milestone was opened for. A history is one order or it is not a history. Per-structure tagging survives, but for O18 (a deletion invalidates the entries that name the deleted data) and for labels, never for ordering.

  **The identity, which is the thing this milestone had to settle:** it is **minted by the registry on registration** and held by whoever registered. Read against O14, which is where the question came from: the arrangement has ids only because `to_document` stamps them, and a structure the client built has none and is not going to be given a stable one for this. The minted handle is also the read-back path the client asked for — the same identity that opened an editable view is the one the edited state is read out through, so "build a structure, edit it in a view, get the data back" is one handle and no correlation table.

  **Acceptance:** the measured defect inverts — two editors over one arrangement, an edit through the first, and `can_undo` is true on the second, whose undo restores the edit the first made; a registry over a structure with no document behind it records, undoes and redoes; a registry over three structures walks one order, which is the order the edits were made in, whatever view made them; registering one structure in a second registry is refused, naming why. A test reproduces the recorded failure (A edits, B edits, A undoes first) and shows it can no longer produce a state that never existed. (The clients are `O19`, which is the pass that proves the packages moved together.)

  **Done 2026-09-01. The module is `history` and it holds no document.** `StructureId`, `Entry` with legs each naming their structure, `History` with one ordered pile and one cursor, and `Spill`/`MemorySpill` moved into it from `log`. `log` is what is left: `Log` is a `History` with one structure registered under the domain name `tree`, and `log::Entry`/`log::Step` state their halves as `Intent`s. **There is one pile implementation**, which is the point — the alternative was the crate carrying two, which is how the same rule comes to behave differently in two places.

  **The identity had to be process-wide, and the test that found it is worth keeping.** A per-history counter gives the second history's first structure the same number as the first's, so `record` accepted a foreign identity and the rule the check exists for passed on exactly the arrangement it refuses. `NEXT_STRUCTURE` is a process-wide atomic; the ids are runtime state and are never serialized, so nothing about a session's format sees them.

  **What the opaque payload cost, and the bug it uncovered.** One pile over several domains cannot hold any domain's type, so a payload is JSON — which the ABI boundary already pays, and which is what buys the property. Spilling then generalized from "a `WriteSamples` over N `f32` values" to "a payload over N serialized **bytes**", and that is where it broke: a span that left for the spill store came back a few ulps from where it went, because **`serde_json`'s default parser is a fast approximation** and exact float parsing is behind its `float_roundtrip` feature. The feature is on now, and it is not the history's concern alone — the same parser reads every session ever saved, so every document that round-tripped through a file was being read back approximately.

- ✅ **O16 - The machinery is generic, the vocabulary is the domain's** *(opened 2026-08-30 with the user)*. `Log` is written against `Intent`, whose four variants — `Place`, `Configure`, `SetMembers`, `WriteSamples` — are the *arrangement's* and all name a node of its tree. The moment a curve or a buffer is a data domain of its own with its own stack, its edits are not those verbs and it has no nodes to name. Widening the enum would put three models' shapes in one type that every reader must handle; so the split is the other one: **the pile is generic, the verbs are the domain's**.

  **What is actually generic** is everything `Log` already holds — `entries`, `cursor`, `budget`, `spill_above`, `spill` — plus coalescing, the transaction and the two directions (back is data, forward may be parameters). What is not is the pair `(state, intent)`: a trait provides `apply(state, intent) -> Outcome` and `current(state, intent) -> Intent` (the inverse read *before* the edit lands, which is what `apply_logged` already does), and `Document`/`Intent` is its first implementor. Since O15's pile holds entries over several structures at once, the pile carries **opaque payloads** and the registry routes each one to the domain its structure was registered with — which is what lets one entry have legs in two domains (O17) without the type knowing either vocabulary.

  **The ABI cost is smaller than it looks, and it is in one place.** An intent already crosses as **JSON bytes**, so a per-domain vocabulary costs no new symbol and no new table row: the tag is in the payload. What is arrangement-specific is the *other* parameter — `clausters_log_apply(h, doc: *mut FfiDocument, intent, …)`. A Rust trait does not cross a C ABI, so inside the crate this is dynamic dispatch and outside the handle stops meaning "document" and starts meaning "the structure, in the registry that holds it". The twelve `clausters_log_*` symbols in `docs/bindings.md` keep their names and their wasm counterparts; their parity tests move with them.

  **A second domain lands with this milestone rather than after it**, and the smallest one is the break-point curve: a trait designed against a single implementor is designed wrong, and the curve is the domain the client will want first (`clients/python/PLAN.md`, "A drawn curve is a list of points, and `Env` is an envelope for `EnvGen`"). Its vocabulary is one verb — the points are now these — which is exactly enough to prove the seam without deciding anything about curves that entry has not decided. It is also the smallest instance of O15's independent element: a curve registered alone, edited, and read back.

  **Acceptance:** two domain kinds share one `Log` implementation; a curve edited through the log inverts to the points it started from; a registry holding a document and a curve records both in one pile and undoes them in order; the arrangement's own suites pass unchanged; `docs/bindings.md` and the C-ABI/wasm parity tests carry whatever moved, and the Python and TypeScript sides bind the same one.

  **Done 2026-09-01.** The seam is `history::Editable` — `apply(payload)`, `current(payload)` and a `coalesce_key` — and `History::apply` is the generic `apply_logged`: it reads the inverse before the edit lands and records only when the structure changed. The arrangement implements it as `log::Tree`, built for one call because what it carries (`against`, `rules`) is the *gesture's* and not the document's; that is why the trait has no room for them, and why a curve is not made to carry them empty. The second domain is `points`: a break-point curve, one verb, and it decides nothing about curves — no shape, no unit — because the only thing a domain owes the seam is how an edit inverts.

  **Three things the ABI pass settled differently from what is written above.**

  - **The symbols were renamed.** This entry said the twelve `clausters_log_*` keep their names; they did not, and the reason is the one the rest of the project applies to every other word: the handle no longer holds one document's undo but one editing context, so `clausters_history_*` is what it is. Since the change was breaking anyway (`CORE_ABI_VERSION` 31 → 32), the rename cost nothing beyond the mechanical pass.
  - **Undo and redo stopped applying, and lost their document argument.** The old shape applied the inverses to the document handle it was given, which cannot survive a history holding structures this surface cannot reach: applying the legs it *could* would leave the rest to the caller out of order, which is how a transaction half-happens. So both hand back each payload with the structure it belongs to (`{"inverses": …}`, `{"edits": …, "remaining": …}`) and the caller applies them. What stayed the crate's is the *decision* — a redo still splits at the first step nobody but the owner can run, and still stops there rather than skipping it. Applying and recording remain one call, because that one has to be.
  - **The coalesce key needed a door of its own.** "The same thing done the same way" is a sentence in a vocabulary, so the pile cannot compute it and a caller recording its own entries has to state it. Spelling `"place:7"` in ctypes and again in TypeScript is exactly the divergence the project forbids, so it is `clausters_document_coalesce_key` — on the *document's* surface, where the sentence belongs.

  **Both clients gained a `History` beside their `Log`**, mirroring the crate one for one: `History` is the pile a composed context registers its structures in, and `Log` is a history with one `tree` in it that still applies the document's legs itself and answers with `{"undone"}` / `{"redone", "remaining"}` — so the editors did not have to change for this milestone, which is `O19`'s work.

  **And a threshold moved with it**: `spill_above` counts serialized bytes rather than `f32` values, everywhere, because the pile holds payloads it does not read and the only size it can measure is the payload's.

- ✅ **O17 - An entry can be a transaction** *(opened 2026-08-30 with the user; the reading-order half moved into O15 on 2026-09-01, where one pile made it nothing)*. One gesture may edit more than one structure — a drag that moves a clip and rewrites the curve it carries — and it has to undo as one step. So an entry is a list of `(structure, intent)` applied in order and inverted in reverse, atomically: if one leg refuses, none of it is recorded. This is not coalescing (O5), which merges *successive* entries on one structure; it is a single entry with several legs, and the two are kept apart in the type so a merge cannot silently join two structures.

  An entry naming several structures is also what settles where a transaction lives: in the registry's one pile, like every other entry, which is the arrangement the earlier per-structure shape could not express without an entry belonging to two stacks at once. What each entry still needs is its **label**, which stops being decoration the moment several structures are on screen: it is how a person knows what a keystroke is about to move.

  **Acceptance:** a composite gesture undoes and redoes in one step and leaves the two structures consistent at every point; a refused leg leaves no entry anywhere; a view over three structures undoes in the order the edits were made, and its label names the entry that is about to go.

  **Done 2026-09-01.** In Rust it is `History::transact`, which takes the legs as `(structure, state, payload)` and is atomic in **both** directions: it reads each inverse before that leg lands, and on a refusal — a rule, a foreign structure, or a leg that cannot state its own inverse — it puts back the legs that already landed and records nothing. The plan said "if one leg refuses, none of it is recorded"; leaving half of it *applied* would have satisfied that sentence and failed the acceptance's real clause, which is that the structures are consistent at every point a reader could look.

  **Across the ABI a transaction is a shape, not a call.** The C surface reaches one document and no curve, so it cannot apply a composite gesture at all — the caller does, and records it. What moved is therefore `clausters_history_record`: it takes the **whole entry** as one JSON request (a label, a coalesce flag, and legs each naming their structure), because a leg at a time is precisely how half a transaction lands. `clausters_document_inverse` came with it, and it is what makes the caller-applies path honest: a leg over the arrangement needs its inverse read *before* the edit, and only the arrangement can state one. `CORE_ABI_VERSION` 32 → 33, with the wasm door, both clients' wrappers and `docs/bindings.md` moving in the same commit.

- ✅ **O18 - What the stack refuses to promise: the non-invertible, the deleted, and the saved** *(opened 2026-08-30 with the user)*. Three cases where a history has to say something rather than pretend, and all three are policy the crate can hold once:

  - **A non-invertible action is recorded as such.** Not every act has an inverse the document can write; an entry that carries none is kept, **marked**, and skipped in both directions — the walk continues past it and says so. Recording it beats dropping it: a hole in the history that announces itself is what lets a person understand why an undo did not go where they expected.
  - **Deleting the data drops its entries — so deleting has to be deferred.** Undoing a deletion must be able to give the data back, so a structure that is out of the tree stays alive while an entry can still restore it, and is really freed when that entry retires (the `budget` already decides when an entry stops existing; what is missing is the hook that says "this one is gone, you may free now"). Deleting also invalidates the entries that **name** the deleted structure — which is what the per-structure tag is for, now that the ordering does not need it: those become non-invertible by the rule above, which is the case that makes the first rule pay for itself. And it is written down as a rule the user will meet: **undoing a deletion returns the data, not its history** — the stack is transient and history is not data.
  - **The save mark.** A save is an event of the registry and the mark is the pile's, so saving stamps one mark, and a structure registered later starts behind it. Crossing it backwards is allowed and **announced**, and the announcement has to be accurate: nothing on disk changed, the file still holds those changes until the next save. Crossing it forward again returns to clean. The reason the warning earns its place is the third case: undo past the mark and then edit, and the redo is truncated — the saved state stops being reachable through the history.

  **Acceptance:** an entry with no inverse is walked past, once, with the walk reporting it; deleting a placed take and undoing restores it, and the underlying buffer is freed only when that entry falls off the budget; entries naming deleted data report as non-invertible rather than failing at apply time; undoing past a save marks the registry dirty and announces it, redoing to the mark clears it, and editing from before it truncates the redo.

  **Done 2026-09-01, and all three turned out to be the same shape**: something the pile must *say* rather than do. A leg with no `backward` marks its entry, and the walk that passes over it hands its label back in `skipped` — so `undo`/`redo` stopped returning a bare list and now carry `label` and `skipped` beside the legs, which is also where `undo_label` moved: it names the entry the walk would actually land on rather than the one next to the cursor. `forget` invalidates the entries naming a structure and answers whether the data may go now; `released` (draining) is the hook the budget was missing. The mark is `mark_saved`/`dirty`/`saved_reachable`, and the third case — undo past the mark, then edit — is the one that needed a *second* question, because after it `dirty` can never go quiet again and a caller has to be able to say why.

  **What the work found: `clear` was losing the release.** Clearing the pile is exactly what makes a forgotten structure freeable — nothing names it any more — and the first version dropped the pending list along with the entries, so the one report the caller frees on never came. The test is in the suite.

  **The GUI host was the only caller that had to change**, and only in shape: `Document::undo`/`redo` read `.intents` instead of a bare vector. Its redo now stops where the crate stopped rather than filtering the steps it could run, which is the same rule stated once instead of twice. `CORE_ABI_VERSION` 33 → 34.

- ✅ **O19 - The clients hold no history** *(opened 2026-08-30 with the user; the client-side landing of O15-O18, kept as a milestone because the packages move together and this is the pass that proves they did)*. `Editor` stops constructing a log and asks for the registry the structure it is editing belongs to; the same in TypeScript. The dedicated views (`open_pianoroll`, `open_signal`) stop being modes that carry a private history and become views over a structure that has one. Nothing about the wire changes and that is worth stating: `"undo"` arrives addressed to the window and needs no widget id, precisely because a window presents one order; `"focus"` already exists if a compound scope ever needs to disambiguate.

  **This is where the generic editor and `edit(x)` are planned, not built** — the verb that opens an editable view over a bare `Timeline`, `Buffer` or curve, the way `plot` opens a static one; the class it returns; and the rename of `Editor` to `FormEditor`, whose model is a tree and which composes the others. Those are the Python and web clients' work and are written in their plans; what this milestone owes them is the floor they stand on, which is that the history is not theirs to hold. The two shapes the clients have to be able to spell are O15's two: **one structure on its own** — built in the client, edited in a view, read back through the handle that opened it — and **several composed in one application**, a `GuiDef` over subviews of different domains whose undo is one order over all of them.

  **Acceptance:** `edit(x)` called twice over one structure gives two windows and one stack, and an undo in either updates both — the inverse of the failure O15 was opened for; the Python and web editors carry no `Log` of their own; a client builds a curve, edits it in a view and reads the edited points back with no composition involved; a `GuiDef` composing a roll and a curve undoes across both in the order the edits were made; the multitrack, the dedicated roll and a standalone curve open together over one composition and undo in one order.

  **Done 2026-09-01, and the log was only half of what an editor was holding.** Sharing a `Log` between two editors fixes nothing on its own: each also derived its **own** `Document` from the same loose objects, so two windows would have stepped one history over two documents. What moved is therefore the whole editing context — the held document, the history over it, the node index, the next id to mint and the version — into an `Editing` (`clausters/gui/editing.py`, `src/gui/editing.ts`) reached through the *element*. `Editor` keeps properties that read it, so the 47 call sites did not move; what it no longer has is a field.

  **A view over a part of the composition reaches the same context.** The derivation already walks the tree to build the index, so the walk claims what it passes — a dedicated roll of one track edits through the piece's history rather than opening a second one over the same notes. It claims only where there is none: a part that already had a context of its own was being edited on its own terms, and taking its history away without being asked is not that walk's to do.

  **What `open` stopped doing is the tell.** It used to close the log and the document when the window was pointed at another composition, on the grounds that an undo must not walk back into a piece the file does not contain. With the context living on the element that is automatic and the closing was wrong: it would have taken the history away from whatever *other* window was still showing the old piece.

  **The `edit(x)` verb is still planned, not built** — the generic editor over a bare `Timeline`, `Buffer` or curve, and the rename of `Editor` to `FormEditor`. What this milestone owed it is the floor, and the floor is there: the history is not the editor's to hold, and `Editing` is what a standalone structure would register itself in.

  **The example pair is `editors/two_windows.py` and `editors/two-windows.html`** — two windows over one piece, drag in one and Ctrl+Z over the other — which is the manual test for the whole track, since nothing in CI runs either.

  **And it is what found the half that was missing.** Run by hand, the example did nothing: dragging in one window moved neither, and Ctrl+Z over the other moved nothing either. Two causes, both worth the record. **One history is not one picture** — an acknowledgement goes to the window whose gesture it answered, so the second view went on drawing a piece that had moved under it, and then its undo stepped an order it could not see, which looks exactly like a dead button. A view now takes its place in the context's list on `open`, and one turn per gesture answers every *other* view on the way out (nested turns collapse, since an `apply` that routes an `"undo"` into `undo` is still one gesture). **As props, not as a redefine** — the first pass redrew the whole window and the two windows then reopened on every step, which is the flicker `_restructure`'s docstring already warns about (*"deliberately not a redraw after every edit"*); a foreign edit reaches the other window the way its own edits do, and only what no prop can carry — a widget that was not there a moment ago, or a turn that changed something and projected no intent — is a redraw. **And the example drove the host wrongly**: it called `pump`, which dispatches to the widget handles a script registered and *consumes* the message, so neither editor ever saw a drag — an editor is driven by `apply`. Both books' composition chapters now say so, because the shape is easy to get wrong in exactly this way.

- ✅ **O20 - Samples and events are domains, not verbs of the arrangement** *(opened 2026-09-02 with the user, out of the "generic editor" direction both clients carry; it stands to that direction exactly as `O19` did — the floor, not the verb)*. `history::Editable` has two implementors, and the second one exists because a trait designed against a single one is designed wrong. What the clients need next is the rest of the set: **a buffer's samples and a timeline's events are structures a person edits with nothing composed behind them**, and today neither is expressible except as a verb of a document. `WriteSamples` names a *node*, so writing samples needs a tree that holds one; `SetMembers` is the same for events. That is why the standalone host can draw over a take and neither client can — the edit only exists as an arrangement's.

  **Two domains beside `points`**, each carrying the one thing a domain owes the seam, which is how an edit inverts:

  - **`samples`** — *this channel's span now holds these values*, inverting to the run it replaced. The host's `"draw"` and `"sample"` payloads already carry both halves, which is the tell that the domain was there before the type was.
  - **`events`** — *the events are now these*, whole, ids kept so what survived an edit is still the same node to a history and to a view. It decides nothing about what an event is, for the reason `points` decides nothing about curves.

  **What that does to the arrangement's own vocabulary.** `Intent::WriteSamples` stops being a fifth verb and becomes a **transaction with a leg in `samples`** — the shape `O17` was built for and that nothing has exercised yet. `SetMembers` keeps its place, since an aggregate's members *are* the tree's, and a roll over a lane records a leg in `events` beside it where the two are one gesture. The line the split states, and it is the one the clients are confused about: **the tree owns where a thing is; the structure owns what is in it.**

  **The identity question the clients raised is answered here rather than twice.** A structure with no document behind it is registered by `History::register` and named by the id that mints; both clients bind that call, neither invents a key of its own.

  **Acceptance:** a buffer's samples edited, inverted and undone with no `Document` anywhere; a timeline's events likewise; a stroke over a placed take recorded as **one** entry with a leg in the tree and a leg in `samples`, undone atomically and leaving both consistent at every point a reader could look; the arrangement's own suites pass unchanged; `docs/bindings.md`, `tests/bindings.rs` and the ctypes parity test carry whatever moved, under a single `CORE_ABI_VERSION` bump for the whole pass.

  **What waits on it:** the Python client's `C50` and the web client's `W28` — `edit(x)` over a bare structure — which is why this is taken first.

  **Done 2026-09-02.** `samples` and `events` are `Editable` beside `points`, and the shape each took is the one its data forced. **`samples` holds nothing**: it is a borrowed view over whoever owns the memory (`Samples::interleaved`), so the crate owns the arithmetic of a strided span — one channel of interleaved frames — and never a second copy of a take, which would have been the largest thing in the process and the one most certainly stale. Its **coalesce key carries the span**, unlike a curve's, where one verb over one structure says everything: a sample dragged twice is one undo, and two strokes over different runs stay two, since merging them would make an undo take back a stroke the hand had already finished. **`events` decides nothing about what an event is** — a position and a payload the crate never reads — because giving one fields would settle "`Track` wraps a `Timeline`" from underneath, and that question is the client's.

  **What the second domain found, which is what a second domain is for: the inverse of `WriteSamples` did nothing.** The document describes where samples are and never what they hold, so the inverse it can state is the **empty write** — and the empty write applied as a no-op, leaving the source's generation where the stroke had put it. Every reader holding a decimation of those samples therefore went on drawing the picture an undo had just taken back, with nothing to tell it otherwise. It now bumps like any other write, which makes the counter **monotonic**: an undo moves it forward, never back, because it answers *is my copy still good* and after an undo it is not. The GUI host's undo path got the fix for free, and one parity vector changed with it — the one whose label read "the same write again is idempotent on the document", a sentence that had quietly turned the inverse into a no-op.

  **The ABI grew one symbol, and it is the coalesce key again.** Everything else a domain needs crosses as JSON and costs no row; the key does not, because "the same thing done the same way" is a sentence *in* a vocabulary, so the pile cannot compute it and a caller recording its own entry has to state it. Left alone, four vocabularies' rules would have been spelled once in ctypes and again in TypeScript. So `domain` is the crate's table of its own vocabularies — the names in one place, `coalesce_key(domain, payload)` answering for any of them — behind `clausters_domain_coalesce_key` / `domainCoalesceKey`, with `clausters_document_coalesce_key` left exactly as it was, since the arrangement's sentence belongs on the arrangement's surface. A domain the crate does not speak answers nothing, which is also what catches a misspelled name that `register` would take in silence. `CORE_ABI_VERSION` 34 → 35 — additive, and the counter moves for the reason v31 records: the ctypes binding declares every symbol eagerly, so a stale library fails at load with a version mismatch rather than an `AttributeError` on a name nobody was looking at.

  **The two clients bind the same one and are checked on it**: the domain names are constants in both, and `document-vectors.json` now carries a row per vocabulary (plus the two answers that are not a key) which `document-parity.test.ts` reads — the project's own idiom for a rule that exists once and is spoken in two languages.

## The notation mode: a sequence that is written, not only played

*Opened 2026-11-08 by the user, while settling how a roll and a page relate.
The roll's half is `crates/clausters-apps/PLAN.md`, `X35` and `X36`; turning
a performance into notation is a module of its own, planned in
`crates/clausters-notate/PLAN.md`.*

**What the user settled.** The roll has two modes, and they are the roll's:
a mode is how the roll reads and edits the events, the sequence holds none,
and the document says nothing of it *(decided by the user, 2026-11-08)*.

- **MIDI**: the events as MIDI says them, in MIDI 1.0, 2.0 or MPE: a note
  is its note on and its note off, its velocity and its channel, with the
  per-note and per-channel curves, and a line played legato is its joins. It
  is the roll as it stands, and it knows nothing of how a page writes what it
  holds.
- **Notation**: the roll is a view of the notation. Its notes belong to a
  staff and a voice, their lengths are written values (a duration is a
  symbol, not a stretch of the time axis), and what they sound is derived
  from what is written: articulations, slurs, dynamics, the pedal. The
  velocity is shown and edited as the dynamic it is.

Three things follow from keeping the two apart:

- **A tie is the page's, never an event's shape.** A note written with ties
  is one event in both modes, as MIDI has one note on and one note off for
  it; the page writes the ties where it has to cut a value (at a barline, or
  where no single figure writes it), as the render of a page already does.
- **A pattern writes MIDI data.** A voice rendered from a pattern writes its
  joins and nothing of a page: deducing ties and slurs from them is reading
  notation into a performance, which is the notation mode's and the
  conversion's, not the render's *(decided by the user, 2026-11-08)*. Two
  notes of one pitch joined are two notes.
- **A polyphony written in one voice** (every note cut and tied wherever
  another note starts or ends) **and a polyphony written in N voices** (one
  voice per staff, or two per staff, voice 1 with its stems up and voice 2
  with its stems down) are the same music notated two ways. Which one a part
  uses is a choice of how it is written, never an edit of what sounds.

**Monophonic or polyphonic** editing is an option of whoever edits
(`crates/clausters-apps/PLAN.md`, `X36`), not a mode and not a property of
the data.

**The part follows a model, so it is the document's.** The staves a part is
written on, the channels it is played on, how its polyphony is written, its
transposition: the views show them and never hold them, and a page and a roll
over one sequence, in either mode, read the same part. **What an instrument
can physically play is not the part's**: the most notes it sounds at once,
its strings, its range, its pedal, which double stops a violin can play and
with what fingering belong to a model of instruments, a module of its own
that is not planned (Future directions, "A model of instruments") *(decided
by the user, 2026-11-08)*.

**What exists under it.** The notation keys an event carries (`staff`,
`voice`, `value`, `pitches`, `articulations`, ...;
`clausters_core::event::notation`), the sequence's `notation` section
(`grid`, `key`, `staves`, `spanners`, `controls`, `marks`, `items`), the
reading of a sequence as a page (`events::transcription::read`), the render
of a page as a sequence (`events::score::render`, through
`clausters_core::notation::Interpretation`) and the write-back of a page's
edit (`events::writeback::write_back`). **What it lacks**: the parts, reading
options stored with the sequence, agreement between the two shapes of "this
note goes on into that one" (`joins`, which MIDI plays, and a slur, which a
page writes), a pattern's render that writes MIDI data alone, a reading that
follows the parts, and a performance derived from what is written.

### The milestones

- ✅ **O35 - The sequence's parts and its reading.** *(Done 2026-11-08:
  `events::part` (`Part`, `Writing`, `Zone`, `EventSequence::parts_read`,
  the `parts` verb), `events::reading` (`Transcription` out of the gated
  module, `Transcription::of` and `over`: what an opener gives goes over the
  stored reading, option by option), `SetParts` and `SetReading`; the read
  names and transposes a staff by its part; the score editor over a
  sequence, the read door of both clients and the multitrack's boxes read
  the stored reading; `parts`, `parts_stated`/`partsStated` and `reading` on
  both clients' `EventSequence`. A line separated from a sequence keeps its
  part. No test opens a box drawn as a page, which needs an engraver; what
  it reads is `Transcription::of`, tested.)*
  - **The parts are a field of the sequence, not of its `notation` section,
    and not behind the `notation` feature**: the roll's MIDI mode reads them
    too (a part's channels), so both modes read the same part. A `Part`
    holds its `name` and `abbr`; how many consecutive staves it is written on
    (each staff's clef, lines and clef changes stay in `notation.staves`);
    its `channels` or its MPE zone, which replaces the reading's "a staff is
    a channel's rank"; its `writing`, `{polyphony: voices | ties,
    voices_per_staff: 1 | 2}`; and its `transpose`. It holds nothing of the
    instrument's physics. The part is where a staff's label, abbreviation and
    transposition come from when a page is read. A sequence that states no
    parts reads as one part per staff, derived and not written back until an
    edit names them.
  - **The reading is stored with the sequence**: `notation.reading` holds the
    options a page is read with (today's `Transcription`: meter, key, clef,
    beat unit, division, tuplets, voices, dynamics), so every opener reads it
    alike. The multitrack's boxes drawn as pages read it rather than
    `Transcription::default()`, which closes the half of `crates/clausters-gui/PLAN.md`,
    "What a score box of the multitrack left open", that says nothing on the
    multitrack states another meter, key or smallest value for a box.
  - **Intents** `SetParts` and `SetReading`, each one undo step, and the same
    fields and intents in both clients' `EventSequence`.
  - **The document holds no mode and no editing option**: MIDI or notation,
    monophonic or polyphonic, are the roll's (`crates/clausters-apps/PLAN.md`,
    `X35`, `X36`) *(decided by the user, 2026-11-08)*.
  - **Acceptance:** sequences and sessions saved before it round-trip byte for
    byte; serde tests for each field and for the derived parts; both clients
    read and write `parts` and `notation.reading`; a multitrack box drawn as a
    page follows its sequence's reading.

- ✅ **O36 - Joins and slurs kept in agreement.** *(Done 2026-11-08: a
  pattern's render writes `joins` alone in both clients; the core's
  `event::notation::articulation` and its bindings are gone (core ABI v97);
  `Spanner.join` in the core model, written in MEI as the slur's `@type`
  (`join`, `attack`) and set by the page's `add_spanner` (a slur drawn on the
  page is attacked unless it says so); `events::articulation` (lines,
  `slurs_from_joins`, `joins_from_slur`, `joins_from_page`, `join`, `unjoin`,
  `prune`); the intents `Join` and `Unjoin` (named so beside the sequence's
  `separate`, which splits it into parts); a render hears a joining slur as
  one line; `write_back` derives the joins from the slurs an edit changed
  and never writes a render's own; `SetEvents` and `Remove` drop a join
  naming a note that is gone, which covers the roll's intake; `join` and
  `unjoin` on both clients' `EventSequence`, and `join` on both editors'
  `spanner`.)* *(Folds in
  `clients/python/PLAN.md`, Found by use, "An edit to a voice's legato or slur
  does not rewrite the other".)* "This note goes on into that one" has two
  shapes: `joins` on a note (the id of the note it goes on from), which the
  playback reads and which is MIDI data, and a slur spanner, which a page
  writes. A voice rendered from a pattern writes today the joins, a `tie` on
  each join of one pitch and a slur over each run that moves, and nothing
  keeps them in agreement.
  - **A pattern's render writes the joins alone**: what `_articulate` does
    in the clients (`clausters/seq/sequence.py`, `src/seq/sequence.ts`) drops
    the ties and the slurs, so a voice rendered from a pattern is MIDI data.
    A sequence saved before keeps the ties and slurs it holds and sounds the
    same, since what sounds is its joins.
  - **A join is an edit of its own** *(decided by the user, 2026-11-08)*: it
    says the next note goes on without a new attack, and only a join gesture
    or a joining slur writes it. A length never does: a note lengthened until
    it reaches the next one is longer, not joined, in either mode.
  - **Whether a slur joins its notes is the editor's choice**, kept on the
    slur: the spanner gains `join`, true when its notes sound as one line
    with no new attack, false when they are phrased legato and each one is
    attacked (its sound is then the slur's `sustain`, as
    `clausters_core::notation::Interpretation` plays it today). The default
    is the editing option it is drawn with: joined where the roll edits
    monophonically, attacked otherwise and on the page; the editor changes
    it. A slur written before `join` existed reads as joined where its notes
    carry `joins`, and as attacked otherwise, so nothing saved sounds
    different.
  - `events::articulation`: the lines of a sequence (its notes grouped by
    `(staff, voice)`, else by channel), `slurs_from_joins` (a run of joins is
    a slur with `join` true, whatever its pitches) and `joins_from_slurs` (the
    slurs with `join` true are the joins; a slur with `join` false writes
    none). The core's `event::notation::articulation`, whose rule makes a
    join of one pitch a tie, goes with its bindings.
  - **Intents** `Join` and `Unjoin`, over the selected notes of one line,
    each one undo step that writes the joins and, where the line is written as
    notation, the slurs; and the slur's `join` set through the page's spanner
    edit, which writes the joins.
  - **The rule: the shape a gesture edited wins**, and the other is derived
    again for the lines the gesture touched, in the same intent and so in the
    same undo step: a join edited (the roll in MIDI mode, `Join`, `Unjoin`)
    derives the slurs of the lines written as notation, and a slur edited (the
    page, the roll in notation mode) derives the joins. Where a sequence
    arrives with the two in disagreement nothing is rewritten: the joins are
    what sounds until an edit touches that line.
  - **Callers**: the roll's intake (`clausters_editing::notes`), which also
    drops a join naming a deleted note; and `write_back`, which stops leaving
    `joins` untouched and derives them for the lines whose slurs the edit
    changed.
  - **Acceptance:** a slur drawn with `join` on a page joins its notes and the
    playback hears one line, and one drawn without it is heard attacked;
    lengthening a note to the next writes no join; a join removed in the roll
    removes the slur it made; deleting a note leaves no join naming it; a
    voice rendered from a pattern writes joins and no tie or slur, the same
    data from Python and from the web client; sequences saved before sound
    the same.

- ✅ **O37 - How a part is written, and the performance derived from it.**
  *(Done 2026-11-09: the reading puts a note on its part's staves by the
  channels the part is played on (a part that names none takes the channels
  no other part claims, one a staff; the last channel of a part with more
  staves than channels splits at middle C), writes as many voices a staff as
  the part says, and writes a part with ties as one voice of chords cut and
  tied where a note starts or ends; `events::realize` and the intent
  `Realize`, refused in a build without the notation layer, with `realize` on
  both clients' `EventSequence`. **A tie holds the pitches two items share**:
  the interpreter, the render's `value`, the MEI writer and reader now follow
  a tie pitch by pitch, so a note tied into a chord goes on while the
  chord's other notes are attacked, and `Marks.tied` names the pitches a tie
  holds where it holds only some of them (a note held under a repeated one);
  `settle_ties` keeps it. A chord merged from two groups is ordered low to
  high, which the property test found. **The voice keys are kept**: under
  ties the reading sets aside the `voice` an event states rather than
  rewriting it, so a part written with ties and back with voices reads its
  voices again and its events are the same, which is the acceptance's
  identity. The property is a seeded generator of 200 sequences, read,
  rendered and read again in both writings.)*
  - **The reading follows the parts**: a staff comes from the part's channels,
    `voices_per_staff` replaces `voices` for that part, and a part written
    with `ties` is read as one voice of chords, each note cut and tied on the
    page where another starts or ends. Without parts the reading is today's,
    unchanged.
  - **Voices or ties is a choice of writing**: changing it changes the part's
    `writing` and the `voice` keys the reading assigns, never `at`, `dur` or
    `sustain`.
  - **The performance is derived**: `realize(sequence, interpretation)` reads
    the sequence, renders the page and writes the difference through
    `write_back`, so the keys that sound (`sustain` from the articulations and
    the slurs, `joins`, the levels from the dynamics, CC64 from a pedal
    spanner) follow from what is written. The roll in notation mode never
    edits them by hand: a velocity edited there is written as a dynamic, and
    the level is derived from it.
  - **Acceptance:** for a sequence written as notation, reading what a page
    renders gives that page back (a property test); writing a part with ties
    and back with voices gives the same events; the two writings sound the
    same.
