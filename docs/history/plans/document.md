# clausters-document - implementation plan: the closed tracks

The tracks of [`crates/clausters-document/PLAN.md`](../../../crates/clausters-document/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

The entries of the plan's "Found by use" and "Future directions" lists that
were fixed or done are at the end of this file, each moved whole once it was
ticked.

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

## Future directions: the entries that were done

- ✅ **The samples vocabulary is read twice, and a blob is the reason** *(named 2026-09-12, closing the routing above)*. `samples::intake` says what a `sample` or a `draw` report means -- the write, and the run it replaced as its inverse -- and the GUI host's `Owner::read_event`/`read_inverse` say it again, in `Intent::WriteSamples`. The two agree today and neither is long, which is exactly the shape a drift takes.

  What stops it being the same call is the **transport**, not the rule: a stroke's run rides the wire as a little-endian `f32` **blob**, and the crate's door takes JSON numbers. The Python client already pays that conversion (`SamplesDomain.request` reads the blob into numbers before asking the crate), so the host could -- at a JSON array per stroke, on the one editing path where the payload is large by design, and then a parse back into a typed `Intent` to apply it.

  The decision is whether the crate grows a door that takes the samples as samples -- which is what a Rust caller holding a decoded run actually has, and what a wasm one holds too -- or whether the host keeps its own reading and the two are held together by a vector test instead. The second is cheaper and states the risk honestly; the first is the rule this file keeps everywhere else.

  **Taken the first way, 2026-09-12.** `samples::write` is the typed door -- the numbers in, the write out -- and `intake` is that same function with the JSON reading in front of it, so a client and a host reach one rule by the road each of them already has. It speaks `f64` because that is what a report's numbers are once read: the wire's samples are `f32`, every value comes from one, both conversions are exact, and neither door has to know which the other used. The host decodes the blob and asks; nothing turns a stroke into a JSON array to be told what it means.

  **And the host was short two rules, which is the answer to whether this mattered.** The copy it kept recorded an **empty stroke** as an edit (a pile entry that undoes to itself) and accepted an **inverse of a different length** than the write it was meant to undo (an entry that leaves half the edit standing). Both are the crate's rules, both are now the only ones. The third difference was the label: `"edit a sample"` and `"draw"` in the host against `"draw the samples"` in the crate, so one gesture read two ways in the undo stack depending on who owned the document. One verb, one name.

- ✅ **The host's owner reads by domain, and today it names the multitrack** *(found 2026-09-12, in a consistency pass over the GUI host requested by the user)*. `crates/clausters-gui/src/host/document.rs` holds two vocabularies in one type. The tree's half is `read_event`, a flat match over `"clip" | "sample" | "mute" | "solo" | "level" | "draw"` that builds `Intent`s; the piece's half is `read_piece_events`, `apply_piece`, `read_clips`, `read_lanes`, `bind_multitrack` and `multitrack()`. The comment on the second says why there are two readers rather than one -- the tree has one verb for a placement and the piece has three -- and that is honest for a host with one application in it.

  `O24` puts three in it. The audio editor arrives asking for `read_samples` and `bind_pane`, the score editor for its own pair, and the type that already carries two vocabularies carries five. The shape that answers this is in the crate already and the host is the one caller that did not take it: `clausters_editing::intake_json` reads a tag against a **domain**, over the four there are, and a tag no domain answers for is refused rather than quietly grown into a fifth vocabulary. `O27` closed by deleting exactly this duplication from `piece.rs`; what stayed is the *routing* above it, still spelled with the multitrack's name in it.

  So: the owner binds a widget to a node **and a domain**, and reads a report by asking that domain -- one door, the applications behind it. What each application then owns is what genuinely differs: which node a gesture addresses and what the picture is redrawn from.

  **Closed 2026-09-12, and the list was two words short.** The dispatch read `tag == "clips" || tag == "lanes"` and the projection answers for **four**: a curve dragged in a host with no client attached reported `points` to a reader with no arm for it, and a `j` reported `join` the same way -- both fell through to the tree's reader and left on the wire, to nobody. So a standalone host drew curves it could not edit and had a join key that did nothing, and neither had ever been noticed, because the path with a client attached is the one that gets used.

  `multitrack::answers` is the vocabulary, declared once where it is implemented, and the host asks it. `multitrack::reading` is the second half: what a report came to, **edits or the reason there are none**, because "the hand changed nothing" and "the piece refused" are the same empty list and opposite things to tell the person who pressed the key. `read` and `intake` are both written on it, so neither door can drop the half the other keeps -- the host was dropping the refusal, which is how the one verb whose answer can be *no* came to be a key that did nothing and said nothing. It now settles the acknowledgement with the reason, which puts the document's own sentence on the window's status bar: the same line a client's `/gui_ack` would have put there, in a host that has no client.

  Held by `the_pieces_own_vocabulary_reaches_the_owner_whole` and `a_piece_that_refuses_a_verb_says_so_on_the_bar`, both against a host drawing a piece.

  **What stayed, and why it is not the same job.** The tree's own reader (`clip`, `mute`, `solo`, `level`) is a vocabulary the crate has no domain for -- it is the description `O21`-`O24` are retiring -- so routing it by domain would mean inventing a fifth domain for something on its way out. The `samples` overlap is real and is filed on its own below.

- ✅ **A clip's fades are its own property, with session defaults** *(decided 2026-09-14 with the user, after reading what the field's multitracks do: Ardour, Cubase, Pro Tools, REAPER, Logic)*. The common model: a fade belongs to a clip's **edges** and is never its gain envelope; a crossfade is what two regions **overlapping** on one track make, join or no join (Ardour: "region fades *are* crossfades", the region beneath heard through the inverse shape); the shapes are at least equal gain (linear, for correlated material) and equal power (the general case); and the length and curve a new fade takes come from a **configurable default** (Cubase's auto fades, 1-500 ms, per project or per track; REAPER's crossfade length and shape on split; Logic's crossfade time and curve for merge and comping). What follows here:

  1. **A fade is a property of the region.** A region's `fade_in`/`fade_out` -- a length and a shape -- is the one object, drawn on the box and moved by a handle. The `gain` automation stays the mix's and never carries a fade.
  2. **The preset is the default of that property, and it is the session's.** It states what a new fade on a clip starts as -- length in ms (10 ms unless the session says otherwise, since every clip is added with one) and shape, equal power or equal gain among the shapes -- and it is written as defaults for a property rather than as a crossfade setting, so the same mechanism can hold other properties' defaults later. It can be **turned off for the whole session**, and **per clip**, where a clip that turns it off keeps butt edges whatever the default says.
  3. **An overlap is a crossfade made out of the fades the clips already have.** Where one clip overlaps the next on a lane, each **stretches its own fade by the time it overlaps the other**: the ending clip's `fade_out` and the beginning clip's `fade_in` both come to cover the overlap, which is symmetric by construction when two clips overlap. Nothing new is stored -- the crossfade is two region fades adjusted -- and it sounds as two readers summed, like any two regions on a lane. It waits for a lane that holds overlapping regions at all.
  4. **Open: more than two clips overlapping at once.** The overlaps can be of different lengths and nest or chain, so "each fade stretches by its overlap" does not say which fade answers to which overlap. To be settled before a lane holds overlapping regions.

  Not a milestone yet because how a fade *inside* a box is drawn is a picture question the multitrack has not had to answer, and it is worth answering once, for a clip's own fades and for the crossfade an overlap makes of them.

  **Done so far** *(2026-10-07, with the join entry below)*: points 1 and 2 as far as sound and picture go — a region's fade is played by its reader and drawn on its box, the default is the multitrack's (`defaults.fade`, 10 ms, `null` off) and a region states 0 to have none — and the picture question above is answered with a line from the silent corner and a veil. **What is still open:** the verbs both clients would set a fade or the default with (the window sends `FadeRegion` now; neither client has a verb for it); a **box of notes**, whose voices no fade reaches; and nothing more of points 3 and 4, which are done below.

  **Specified further by the user, 2026-10-07**, after seeing the first pass: a region's fade in and fade out are **one trapezoidal envelope** over the box, its two sloped sides with selectable shapes, and edited with the same logic as a clip's automation (its points) while behaving as its own thing — not the amplitude automation; the default shape is **equal power** (quick at first, slow at the end of a rise), selectable; the default length 10 ms; and **two regions that overlap crossfade** through their fades, longer as they overlap more, with the edge cases settled.

  **The shape, done the same day.** `Fade` carries an envelope segment's shape — the `envshape` vocabulary every `Env` and automation segment speaks, by name in the file (`"wel"`, `"lin"`, `"curve"` with its `curve`) — and `wel`, equal power, is the default. The reader plays each side through `EnvShape`, a new UGen that evaluates `envshape::shape_value` at a phase rather than over time (a fade follows the transport's position, which a locate jumps and a stop holds), and the host draws each side traced through the same function, so the curve on the box is the one heard. Both clients' `Fade` became `Fade(length, shape="wel", curve=0.0)` (it carried an opaque shape nobody read), and both bind `env_shape`/`envShape`. Two fades longer than their region together are cut in proportion, so the trapezoid becomes a triangle and never a dip. **Left:** a join's seams are still played as straight ramps — the stitch on the server (`dsp::stitch`) has a fade length per part and no shape, so carrying the shape there is a change to `/buffer_stitch`.

  **The handle, done 2026-10-08.** A box's fades are drawn and edited as an envelope layer of the box, `fade:<region>` (`picture::FADE_LAYER`): the four points of the trapezoid (`picture::fade_points`), each sloped side's shape on the point that starts it, projected beside the clip automations (`editing::fade_layers`/`fade_points`) and edited with the very curve element a clip automation is, under a trapezoid's rules (`curve`'s `trapezoid`: the ends cannot be taken, the corners of the top move in time only and not past each other, no point is added or removed, and the flat top is the box's, so a press on it still moves the box). A report of the layer comes back as the region's fades (`read_fades`): a side that moved is written as the region's own, one that did not stays as the region states it, so a region on the default keeps following it, and a report that moved nothing is no edit. The silenced part is veiled by the box, read off the same layer (the `fades` prop that first carried it lagged behind a drag — the user saw the veil stay put while the curve moved — and is gone).

  **The crossfade, done 2026-10-08** — points 3 and 4 of this entry. `TakeLane::edges` is the one reading of a region's edges on its lane, for the player, the picture and the layer: where a neighbour starts before a region and ends inside it, the region's fade in stretches to cover the overlap, and where one starts inside it and ends after it, its fade out does, so the earlier region's fade out and the later one's fade in cover the same span and the crossfade is as long as the overlap. Nothing is stored: move either and it follows. The edge cases, decided: a stretch never shortens a fade a region states; a region inside another, and two that start or end on the same instant, keep their own fades, since neither has a side to fade out on while the other fades in; a muted region crosses with nobody; with several neighbours each edge takes the longest overlap that reaches it — which answers point 4 without choosing a pair; and two fades that would cross inside a region are cut in proportion and meet. Heard in `an_overlap_crossfades_at_equal_power`: two constant takes are each at sin(pi/4) half way through the overlap. Still open here: a region's fade dragged shorter than the overlap it is stretched to snaps back to the overlap, since the overlap wins.

  **Answered in the window, and a switch** *(2026-10-08, from the user's use)*. The crossfade was played and not seen: a drag that applies cleanly is acknowledged and the window keeps its own picture, so the two boxes kept their short fades where the crossfade should have spanned the overlap. The multitrack editor now answers an edit with the layers and points it moved (`derived`), unless `settle` is about to send the whole picture anyway. And the automatic crossfade is a facility that can be turned off (asked by the user): `Defaults::crossfade`, on unless a file says otherwise, set with `MultitrackIntent::SetDefaults` — the defaults stated whole, undone like any edit — not from the menu: the state is the document's and the menu is one more caller. Both clients set it as `Multitrack.crossfade` (Python and TS, a property whose setter is the same `setdefaults` edit, so a script's change is recorded and drawn like the menu's); the Edit menu's "Crossfade overlaps, on or off" sends the window's `crossfade` verb, which the multitrack editor turns into that edit. Off, overlapping regions keep the fades they state and are two regions summed. A region's own fades already had their verbs in both clients (`Region.fade_in`/`fade_out`); what is left of the verbs is the default fade, `defaults.fade`, which no client sets yet.

  **The rest, done 2026-10-08.** *The default's verb*: both clients set it as `Multitrack.default_fade` (`defaultFade` in TS), a `Fade` or none, through the same `setdefaults` edit. *A box of notes fades*: its voice bus enters its clip through the reader's own envelope (`mixer::input_def` takes `at`, `span` and the fades, handed by the plan as `PlannedClip::edges`), silent outside the box as a reader's gate is, and its `fade:` layer is drawn and edited as a box of samples' is. *A fade dragged shorter than its overlap*: **only an edge on the default is stretched** (`TakeLane::edges`) — an edge the region states, which a drag writes, is the region's word and is kept, shorter or longer than the overlap, while the other region's edge, still on the default, goes on covering it. Heard in `a_box_of_notes_fades_at_its_own_edges`. What stays open of fades is in the two entries below about the curve heard at a seam and the switch's reach.

- ✅ **A join takes its clips' fades, and invents none** *(raised by the user 2026-09-12, on the first working join: "you added a cross fade to the join; it is something that is there but cannot be seen, and it should; it can be a join option, but it has to be specified and configurable"; decided 2026-09-14, with the entry above)*. `O31` fades every seam **where the material is cut** -- 10 ms, `picture::SEAM`, and nothing where two parts read on from each other -- and neither half was asked for. It is **not seen**: the fade is in the source and a box is drawn as a window onto a source, so the picture shows a join and not the shape of its seams, where a region's own fades are drawn where they are. And it is **not stated**: 10 ms is a number this crate chose in a place with no vocabulary for choosing one.

  **The rule as it stands today, written down so it can be found and changed** *(`crates/clausters-document/src/multitrack/picture.rs`, `SEAM` and `read_join`)*:

  1. A seam between two parts is faded **only where the material is cut** -- where the next part does not read on from where the one before it stops, or reads a different source. Two consecutive pieces of one take get **no** fade.
  2. The fade is **10 ms**, linear, on both sides of the seam: `fade_out` on the part that ends and `fade_in` on the one that begins.
  3. It is the same number in every client, because the join carries it -- the one part of this that stays: whatever the fade is, it travels with the source, so nothing can disagree about it.

  **What replaces it.** A clip's fades are its own (the entry above), so a join has nothing to decide about them:

  - **A join of juxtaposed clips adds nothing.** Every clip is added with the default fade, so two clips edge to edge already have a `fade_out` and a `fade_in` at the seam; the join carries each region's fades into its parts, and `picture::SEAM` goes. *(Corrected by the user the same day: an earlier reading had the join add a symmetric crossfade sized from the two parts.)*
  - **Open: a join of overlapping clips cannot be one stitch as the server reads one today** *(found by the user 2026-09-14)*. A join's segments would overlap **in samples**, and each overlapping segment needs its own fade applied while the other sounds -- which is mixing, not reading. `/buffer_stitch` cannot say it: its parts are laid **end to end** (`src/dsp/stitch.rs`: a join's length is the sum of its parts, and no part states where in the join it starts), and a reader holds **one part per run** (`Stitch::run_at`), so exactly one part sounds at any frame. Whatever is chosen also has to answer more than two clips overlapping (the entry above, point 4). Three ways, to be decided later:

     - **A. The stitch learns overlaps; one reader per box stays.** Each part gains its position in the join, and a read in an overlapped span sums the parts that cover it, each through its own fade. *For*: it keeps `O31`'s decision whole -- the node plan, the instance and the picture still see one source and one reader per box. *Against*: a mix lives inside what is meant to read like a buffer; the one-part-per-block fast path becomes up to N parts wherever parts overlap; the `/buffer_stitch` wire and the session's `Location::Segments` both gain a position per part (a `FORMAT` bump); and once takes play out of `DiskIn`, that one reader has to read several file positions at once.
     - **B. An overlapped join is layers, each with its own reader.** The segments are split into layers with no overlap inside any of them -- as many as the deepest overlap, which is interval colouring -- each layer an ordinary stitch read by its own reader, summed in the box's slot. *For*: it is the shape two overlapping regions on a lane already have (two readers); the stitch read is unchanged; and colouring answers the more-than-two case structurally, each part keeping its fade. *Against*: a box can have N readers, which the node plan, the instance and the picture all have to learn -- the very thing `O31` avoided; and a layer has gaps, and a stitch with silence in it does not exist on the server either (`O31`, "What it deliberately does not do yet"). A join of juxtaposed clips is one layer and would not change.
     - **C. No overlapped join as one source.** Either the join over overlapping regions is **refused** -- they stay separate regions, heard through their own readers and fades -- or the overlapped span is **rendered** into a new take, as a consolidate does, which writes samples and stops being non-destructive.

     Leaning noted without deciding: B keeps an overlap where the rest of the system already puts one, in several readers summed in the graph, and solves the more-than-two case in the structure; A keeps "one reader per box". To be taken before a join over overlapping regions is allowed.

  **Done 2026-10-07, for a join of boxes that meet**, and it took the entry above's first two points with it, since a join that adds nothing is only silent where the clips already have fades. A region's fades are now **played** — the reader ramps the box's own edges (`mixer::FADE_IN`/`FADE_OUT`, off `TransportPos`) — and **drawn**: a line from the silent corner to full level with the silenced part veiled, from a `fades` prop the projection fills with the same numbers the plan plays. A region that states no fade takes **the multitrack's default** (`Multitrack.defaults.fade`, 10 ms when the file names none, `null` to turn it off for the session; a region turns it off for itself with a fade of length 0), so every clip has one without any writer adding it; `Region::fades` is the one reading of the rule, for the reader, the picture and the join. The join carries each box's fade at an edge it makes inner into the part that edge begins or ends, and keeps the joined box's outer fades as that box's own; `picture::SEAM` is gone. **A cut where nothing moved is not heard**: `SplitRegion` leaves butt edges at the cut, and so does a cut a window reports — a shortened box and a new one reading on from it in the same source (`butt_cuts`) — so the two halves meet as the recording did, and a join that puts them back adds no dip.

  What it changed in what is heard, deliberately: every box of samples in every saved session now fades in and out over 10 ms unless it says otherwise, where before nothing faded but a join's seams. Recorded here because a session that relied on a hard edge — a percussive take starting on its first frame — has to state it with a fade of length 0.

  The join of **overlapping** boxes is still refused, and the three ways above are its own entry now, below.

- ✅ **Which map a region in beats is read through** *(named 2026-09-17, placing
  the multitrack in seconds -- `clients/python/PLAN.md`, the tempo entry,
  decision 8; **dissolved the same day** by the user)*. The question assumed a
  region could hold contents in beats -- a composite region, or a window onto a
  node of the general tree -- and asked which tempo map would convert them. Its
  premise is wrong: the multitrack is a model of its own, with no nodes as the
  `clausters.form` tree has, so no region of it holds beats and there is
  nothing to convert. What made it look like a question is a leftover of
  `form` inside the multitrack document, filed under "Found by use" ("A
  composite region is `form`'s tree inside the multitrack").

- ✅ **A track is as wide as its widest region** *(decided with the user,
  2026-10-28, settling the audio editor's channel rules)*. Editing a file's
  channels and holding regions of different widths on one track are not the
  same problem. A region on a track plays by the audio editor's implicit map
  (a mono region on every channel of the track, otherwise channel by
  channel); a track takes the width of its widest region rather than a fixed
  two (`Track::channels` stays the field that says it, so the mix survives a
  save), and the channels it has past the multitrack's (`Multitrack::channels`)
  are routed: to the master's first channels unless the track says where.
  The routing is the channel matrix the audio editor's paste rules build
  (`crates/clausters-apps/PLAN.md`, "A file of fewer channels is drawn
  silent where it is played"), and it is also the N-to-M table the
  mixer's strip refuses today for widths past stereo; the strip's pan and
  balance stay its own controls, outside the matrix.
  Lands with the multitrack's port onto the audio editor's modules (`X34`,
  `crates/clausters-apps/PLAN.md`).
  *Done 2026-10-29:* a track's take is read as wide as its widest region,
  up to eight channels (`mixer::MAX_TRACK_CHANNELS`); past stereo its strip is
  gains with no image, its meter a column per channel, and its way into the
  master `mt.route.N.M`, the folded matrix (`ChannelMatrix::folded`): the
  channels the master has read the track's of the same number, and those past
  them land on the master's first ones again. A track saying where its
  channels go is not written yet; the folded routing is what it says today.

- ✅ **The pitch a sequence sounds over time** *(2026-11-10, with the
  user, from `crates/clausters-apps/PLAN.md`, `X41`; done the same day as
  `events::sounding::pitch_runs`, the `pitchline` door)*. A function of the
  sequence, beside `articulation`'s lines: for each line, the pitch that
  sounds as a run of points in beats and fractional MIDI numbers -- each
  note's pitch, its own bend curve over it and, under MIDI 1.0, its
  channel's bend over the time it sounds -- one run across notes a join
  carries on into, and a new run where the next is attacked. The roll draws
  it over its boxes, the score editor can read it, and both clients reach
  it through the sequence's JSON door. A channel's bend stays its own
  curve; this is only a reading of what sounds.

## Found by use: the entries that were fixed

- ✅ **The master freezes with the transport** *(found 2026-09-23 by the user,
  while designing the audio editor's nodes: the master's meter, the master and
  its send must not be frozen, and neither must the master input, the input's
  own master and its meter)*. The multitrack's instance is created inside the group it binds
  with `/transport_group` (`clausters_editing::apply`, `Op::Transport`), and the
  multitrack graph holds the master strip, the master's meter and the send to
  the hardware (`clausters_core::mixer::multitrack_graph`). So a pause freezes
  them with the readers: the master's meter holds its last reading instead of
  falling, and nothing after the tracks can ramp across a stop. **To do:** only
  what follows the transport is governed -- the tracks, their clips and their
  readers -- and the master, its meter and its send stay outside that group,
  under a parent that owns both. It is the shape the audio editor's nodes take
  in `crates/clausters-apps/PLAN.md`, `X7`, and the transport's stopping phase
  that `X7` needs for its declick serves this one too.
  **Fixed 2026-09-24.** The tracks are made in a slot of the multitrack's graph
  (`mixer::tracks_graph`, the `transport` slot), and that slot is the group the
  transport governs; the group made at the top **follows** transport 0 and
  holds the graph, so the master strip, its meter slot and its way out run
  through a stop. The way out is `mt.out`, the send times `TransportFade`, and the
  playback gives transport 0 a 5 ms ramp, so a stop and a play fade the whole
  mix at one place and nothing clicks. A track's meter, still governed, closes
  itself on the ramp (`mt.trackmeter`), so a frozen track reads zero rather
  than the level it froze on. The containment stays: the master still hands
  every track its mix bus. `tests/mixer_graph.rs` hears it, and the user heard
  the multitrack's example play and pause with no click. The master's meter is
  a slot nothing fills yet -- "The multitrack has no master in its view", in
  "Future directions".

- ✅ **A box longer than its source holds the source's last sample** *(found
  2026-09-23 while measuring the audio editor's example; the user asked for it
  to be fixed on its own, separately from the audio editor's nodes)*. The
  multitrack's reader (`clausters_core::mixer::reader_def`) gates on the box's
  window (`at`, `span`) and nothing else. A box whose window reaches past the
  end of its buffer -- a take made shorter by an edit, a `span` stated longer
  than the source -- reads past the last frame, and `BufRd` without a loop
  clamps there, as it does in scsynth. The last sample stays on the track's bus as a
  constant until the window closes. No UGen may leave a DC offset on the
  output (the user's rule). **To do:** decide where the end is enforced --
  the planner never states a `span` past what is left of the source
  (`multitrack::nodes::plan`), or the reader's gate also closes at the buffer's
  end -- keeping in mind that the gate runs per sample in every reader. A
  first fix gated on `BufFrames` in the reader and was reverted the same day
  for that per-sample cost. Test: a box of sixteen blocks over a take of four is
  heard for four, and the rest is exactly zero.
  **Fixed 2026-09-24, in the planner.** A source's entry in the table says how
  long it lasts (`SourceInfo::duration`, in its own seconds, optional), and
  `plan` cuts a box that does not loop at what is left of its source past the
  window's start, read at its playrate; the reader is untouched. Every endpoint
  states it: the standalone host from a take's frames and rate, both clients
  from the buffer they hold (`Sources.table`). A source whose length nobody
  states is not cut. `tests/multitrack_playback.rs` is the test above, through
  the playback.

- ✅ **A composite region is `form`'s tree inside the multitrack** *(found
  2026-09-17 by the user, while the scores phase asked which map converts the
  beats inside a region: "A composite does not exist in a multitrack because
  there are no nodes as in the form module. The multitrack is a completely
  different model.")*. `multitrack::Content::Composite { node }` places a
  general-tree `Node` as one region, and both clients bind it
  (`Content.composite`); only tests use it, and `MultitrackPlayback` skips it.
  It is what is left of the arrangement that was `clausters.form`'s, and it let
  a region appear to hold beats -- a `Sequence`, a `Clang`, an `Aggregate` --
  in a document whose regions are sources placed in seconds. **To do:** remove
  `Composite` from the multitrack document, or move it to another kind of
  document if the `form` module is taken up again; the logic can live in an
  editor specific to `form`, never in the multitrack. The same pass reviews the
  window onto a node (`SegmentSource::Node`, a window onto a node of
  `Document::content`), which has the same origin, and the migration of a
  session that saved either.

  **Removed 2026-10-07.** `Content::Composite` is gone from the crate and
  `Content.composite` from both clients. A region's fill is a window, or one
  this build does not know.

  **The window onto a node, reviewed: it is the general tree's and stays
  there; what goes is a *region* being one.** `SegmentSource::Node` is how a
  `Body::Segments` of the general document reads a node of `Document::content`
  — a cut of a timeline of notes is two windows onto one node instead of two
  copies — and that document is alive on its own account: the document door,
  its log and its history, and the host's owner all hold one. What had the
  composite's origin was the multitrack borrowing that window type, which let
  a region name a node of a tree the multitrack does not have. So the
  multitrack has a window of its own, `multitrack::Window`, over a `SourceRef`
  and nothing else. The type says what the user said: no region reads a node.
  On the wire it is the object a window onto samples always was, so nothing a
  session wrote for a source moves; the readers that asked "is this onto
  samples at all" (`window.source.samples()`, in the plan, the picture, the
  loader and the host) stopped asking.

  **No migration, as decided** *(the user, 2026-10-07: a multitrack keeps
  only its own sessions, with no `form` in them)*. A session that wrote a composite or a
  window onto a node still opens: both read as `Content::Unknown`, the fill
  this build does not know, which is carried whole and written back as it came
  — the region keeps its place and its length, draws as a named box and plays
  nothing. Nothing converts it and nothing drops it. Held by
  `a_composite_and_a_window_onto_a_node_are_fills_it_does_not_know`, and by
  one test in each client that a composite read from a file is written back
  whole and is no window.

  The shared vectors lost their composite regions (the multitrack's, its
  session's and the document door's), which are windows onto sources now.
  `Session::document` stays — the general tree, beside the multitrack — and
  its doc no longer says a composite region is what replaces it.

- ✅ **Boxes that met on a sample stopped meeting once the multitrack was in seconds** *(found 2026-09-17 by the user, the day it landed: a snap that sometimes missed, a join refused, and in the editor example)*. The projection crossed a length on its own (`dur / rate`) and an end as `(at + dur) / rate`, which differ in the last bit for most sample counts, so a box's end and the next box's start stopped being equal and `read_join` -- comparing seconds against `f64::EPSILON` -- saw a gap. Beats had hidden it by computing a length as the difference of two converted positions. **Fixed** by one rule for every crossing, in the core (`tempoclock::secs_to_samples_over`/`samples_to_secs_over`): a position lands on a whole sample and a length is the difference of its two ends; a join asks where boxes meet in samples at the view's rate. The host also reported positions as `f32`, which holds a whole sample only up to 2^24 (under six minutes at 48 kHz); the `clips` and `points` reports are doubles now. Held by `halves_placed_at_any_sample_draw_back_there_and_join` and the core's `a_span_crosses_as_the_difference_of_its_ends`.

- ✅ **A box drawn a different length than its samples** *(reported 2026-09-17 by the user, with a screenshot, in the same message as the entry above: "the drawing of the samples does not match the length of the clips' box as before")*. In `clients/python/examples/editors/edit_multitrack.py`, the box's picture and its span no longer agree. Not reproduced: the gestures that led there are not known yet, and nothing in the crossing fix above explains a difference that large.

  **Gone, confirmed by the user 2026-09-22; read the path before closing it.**
  The whole path, since it is spread over four places and this entry is what
  names them: a box's rectangle is `graphics/track.rs` -- `clip_x_range` puts
  `offset`/`dur` through the navigation window, `clip_rect` is the rectangle,
  `clip_local_view` is the slice of `[0, dur]` the rectangle shows, and
  `clip_ends_on_screen`/`clip_grips` are the two grips, which the renderer and
  the hit test both call so that the strip that lights up is the strip that
  resizes. Its **contents** are `elements/multitrack/draw.rs::paint`, which
  hands the body the box's own axis (`TimeSpace::of(local, dur)`) and its window
  onto the source (`SourceWindow { start, looping, fit: false }`), or
  `draw_spans` for a join. And the **stretch** is `structures/boxes.rs::drag`,
  one function for every box with a time axis -- a multitrack's boxes and a
  roll's notes alike -- whose rules are: the end edge moves only the duration,
  the start edge trims by carrying `start` with it, a drag never leaves less
  than `MIN_DUR` (one sample, a length in the axis' units and never a count of
  pixels), never passes `Bounds::limit` (the far edge of the domain), and never
  passes `Contents::total` unless the box loops. `fit: false` is the sentence
  the drawing and the drag agree on: **an edge drag is a trim**, the samples are
  read frame for sample, and nothing here stretches material.

  **So the picture and the box can only agree while one sample of the box is
  one frame of the source**, and the two candidates for this report are both
  breaks of that. The **join's edge**: a join's own buffer is not fetched, so
  `contents_of` answered no length and `drag` bounded its edge by nothing until
  2026-09-21 (`crates/clausters-gui/PLAN.md`, "A box can be stretched past the samples it
  has") -- a join pulled out is drawn exactly as long as it is and reads the
  spans it has, which is this picture. The **report's numbers**: `json_arg` sent
  a box's placement back as `f32`, exact for whole samples only up to 2^24 of
  them (under six minutes at 48 kHz), which was harmless while a placement on
  the wire was a count of beats and stopped being so the day it became a count
  of samples; they are doubles since the crossing fix above, which landed the
  same day this was reported and said it explained nothing. Which of the two it
  was is not established, and the entry closes on the picture agreeing rather
  than on a cause.

  **What the reading turned up is the entry below** -- the rate a box is
  measured in is the view's and the frames behind it are the source's, and
  nothing converts between them.

- ✅ **A join is a flat list of segments, and a segment never names a join** *(decided 2026-09-13 by the user, after joins of joins nested past the server's four levels -- `crates/clausters-gui/PLAN.md`, "Found by use")*. The model, in the user's words: a segment is a read-only two-dimensional pointer into a buffer or a file, `{ source, start, length }`, and a join makes a new object that is a list of those; no recursion. So a `Location::Segments` part names a **take**, never another source that is itself segments. `picture::read_join` holds that by construction: a box over a join reads through to the join's parts, cut to what the box shows (`segments_of`), and a part that states no range, or a span past the end of the join, is refused rather than joined around. What a caller has to know for it is the segments of the joins it holds (`Buffers::parts`); the multitrack editor learns them from the joins it mints, since a join is never edited. A session saved before this may still hold a join over a join, which is read one level through and not repaired.

- ✅ **After a trim, a window's duration no longer says what its box plays** *(decided with the user 2026-09-13; see the note at its end)* *(found 2026-09-13, diagnosing a join that came out empty -- `crates/clausters-gui/PLAN.md`, "Found by use", "A join over more than two segments at once fails")*. `picture::read` answers a trim by sliding `window.start` and leaving `window.duration` as the piece had it, on the grounds that a flat report says how long a box is and not how much of the source is behind it. So after a left-hand trim, or on the left half of a split, a window claims more of its source than the box plays -- even past the end of the take. Nothing heard reads the duration (`nodes::plan` plays a box by its position, its length and its window's start) and nothing drawn does, which is why it stayed invisible; `read_join` did, and asked the server for samples that were not there. The join now reads what the box shows. What is still open is **what the field means**: either a trim keeps it equal to what the box shows (and a saved piece stops carrying a number that lies), or it is stated as meaningful only for a box that wraps, where a window shorter than its box is exactly what looping says. The first changes what an edit writes into the format, so it is a decision, not a fix.

  **Decided: neither.** The user's model: a region is a view onto its **whole** source, so a trim hides and pulling the edge back shows (and sounds) again what it hid; keeping the duration equal to what the box shows would throw that away. A join is the more specific case: it turns boxes, trimmed or not, into one box whose source is exactly the sum of its segments, so its edges cannot be pulled past them. So `SegmentRef::duration`, in a region's window, is **how much of the source the window reaches** -- the whole take, or the sum of a join's segments -- and never what the box shows; a trim or a split moves `start`, position and length and leaves it alone, and `start + duration` is not the end of anything that plays. That is what trims already wrote; what did not follow it was `picture::read` minting a box a hand made with what the box showed, and it now writes the source's length where the caller knows it (`reach_of`, from the table's frames). The total length of a take is the **source's** fact and lives in the source table (`Source.frames`), not repeated in each segment -- a segment stays `{source, range}`, read-only, and finds its take's length by id, which is also what a file read with `DiskIn` will need, with no buffer to ask. A standalone host now writes that length down once its takes are mapped (`Owner::learn_lengths`), into both its take table and the session it saves; a client states it with `Source.shaped`, as the multitrack example does. Held by `a_new_box_windows_its_whole_source`.

- ✅ **A second join left an empty box, because a source the piece stopped naming is still a source** *(found 2026-09-12 by the user: "cutting several segments, resizing them, shuffling them and selecting them all, on join an empty clip is left"; fixed the same day)*. `fresh_source` minted off the **piece** alone — the largest source id any region windows, plus one — which is how every other id here is minted and is wrong for this one. A source stops being named by the piece the moment nothing windows it (an undo, a box deleted, a joined box cut back up) while the client still holds the buffer it made for it. So the next join was handed an id that already had samples behind it: the client saw an id it knew, made nothing, and the box became a window onto **the previous join** — or onto a buffer that had since been freed, which is the empty box.

  The piece cannot see the table, so the table says: `Buffers::taken` is the question "which sources do you hold samples for", defaulted to none so a caller with no table is still one, and the mint clears the piece **and** it. The user put the rule in one line while it was being fixed — *"joined clips should create new instances of segments; the implementation that way should be simple"* — and that is what makes it simple: a join never reuses, so nothing has to reason about whether two joins are the same join. A **redo** naming the same id is not reuse; it is the same edit.

  **The same pass closed the other half of the shape.** `read_join` *skipped* a name it could not resolve and joined the rest, which would leave that box where it was, under the box now spanning over it. It refuses instead. Every bug this seam has produced has been a verb quietly acting on less than it was given.

- ✅ **A box that went took its envelope's map with it, and the reconciler named it anyway** *(found 2026-09-12 by the user, deleting a box in `edit_multitrack` the first time the instance projection met a hand)*. `reap_curves` emitted an `Unmap` for the port a deleted clip's curve drove -- after the pass that freed that clip. Freeing a node takes its map with it, so the message was about a node the server no longer had; worse, it named a **handle** whose table entry went with the free, which in Python raised a `KeyError` and in the page would have resolved to `0` and sent `/graph_map` to the **root group**.

  Two halves. The crate does not emit it: a curve remembers whether its owner is a box or a track and which id, so the reaper asks whether the owner is still there. What the curve holds of its own -- its node, its buffer, its bus -- is given back either way, since those are the instance's and outlive whatever port they drove. And both clients now answer an impossible handle the same way, by skipping: one raising while the other addressed node zero is the divergence this track exists to retire.

  It is the argument for handles rather than against them. A node id would have been a number the server quietly ignored or, worse, one it did not; a handle with nothing behind it is a question with no answer, which is a thing a client can *see*.

- ✅ **A break-point compared by how its data was spelled, so a page saw an edit where a script saw none** *(found 2026-09-11 by the parity vectors of `O27`, the first time the same report went through both doors and the answers were put side by side)*. `read_points` asked `held.points != curve.points`, which is derived equality over `Opaque(Value)` -- and a JSON number compares by **representation**: JavaScript writes `0.0` as `0` and Python writes it as `0.0`, so a curve reported back exactly as it was drawn came out as a `SetAutomation` in the page and as nothing at all in a script. Nothing failed loudly: the page simply recorded an undo entry per redraw of an untouched curve.

  It is worth reading as a class rather than as a bug. A point's `data` is **opaque on purpose** -- the document carries it and never reads it, which is what lets a client keep a shape there -- and opaque data travels through whichever serializer the endpoint has. So any comparison of it has to be by what it *says*. Fixed with `same_points`/`same_json` beside `read_points`, numeric-aware and otherwise the equality it always was, with a test that states the rule.

  **And it is the argument for the vectors themselves.** Two clients disagreeing about whether a gesture was an edit is not a build failure, not a type error and not a lint; it took generating the same question from one client and asking it from the other.

- ✅ **A window could only be onto samples, so a cut over notes was a copy**
  *(found 2026-09-03 by the user, on the split over notes that shipped the same
  day: "when a piano roll is split, is it still a window onto the original
  data?" -- it was, until it was written down; fixed the same day)*.
  `SegmentRef.source` was a `SourceRef` and nothing else, so the only contents a
  window could be onto was samples. That is enough while what a window reads
  lives *outside* the document -- two windows are two references and nothing is
  copied -- and it is not enough for a timeline of notes, whose notes are
  **nodes**: two windows written as two tracks wrote every note twice, with the
  same ids in each, which is one identity under two parents. It passed
  `duplicate_id` only because the copies were identical (the check refuses two
  *different* nodes under one id, deliberately), and it came apart on reopening,
  where each half got a timeline of its own.

  Two additions, and the second is what the first needs:
  **`SegmentSource`** -- a window is onto `Samples(SourceRef)` or onto a
  `Node { node }` of this document -- and **`Document::content`**, where a node
  a window names lives. Content is not placement, so it is not in the tree; it
  *is* in the document, in the same id space, which is what lets a window and an
  intent name a node the same way. `Body::Segments::duration_unit` is derived
  from the sources rather than fixed at `Seconds`, the finders an intent uses
  reach content as well as the tree (without that, no edit could touch shared
  material), and `duplicate_id`/`max_id`/a session's dangling scan all cover
  both halves.

  The wire is unchanged for everything that existed: samples are the object a
  `SourceRef` already was, a node is `{"node": id}`, and `content` is skipped
  when it is empty -- so every document written before this reads back
  identical. What decided the shape, over the two alternatives the user weighed,
  is in `docs/decisions.md`.

- ✅ **The document measures two kinds of leaf with one unit, and for one of
  them the unit is wrong** *(found 2026-08-30 by the user, arguing that concrete
  time is seconds or samples and that beats are a ruler and a snap the editor
  imposes)*. `Beats = f64` is the unit of `Node.onset`, `Node.duration` and
  `Intent::Place.offset`, for every body. **Storing a quantity in beats is a
  statement**: it says *the seconds this is worth must follow the tempo*, since
  the stored number never changes and only its worth in seconds does. For a
  clang that is exactly right — a note is musical, and a tempo change is
  supposed to shorten it. For a leaf that references samples it is false: a
  take's length is `frames / sample_rate`, a wall-clock fact already fixed
  before the document saw it, and keeping it in beats would need the number
  rewritten at every tempo change, which nothing does.

  **The rule, as the user stated it**, and it is two rules rather than one
  because onset and duration answer to different things:

  - **An onset is in the unit of what contains it.** Concrete material inside a
    sequence measured in beats has its onset in beats — the placement is a
    musical decision and follows the grid it was placed on.
  - **A duration is in the unit of the material.** Audio is seconds; a sequence
    of events is beats. Concrete material is one of those two, and its own
    nature says which.

  **The document already knows which is which** and needs no new field: the body
  says it. `Clang` and `Sequence` are events; `Vector` and `Segments` reference
  samples. So the unit of a duration is derived from the body kind, and only
  `Beats` as the name of every temporal field has to give.

  **And the conversion belongs to the flattening, not to the structure.** A
  timeline is a list ordered by *one* number, so it cannot hold items in two
  bases: the tree keeps each leaf in the unit that leaf is in, and
  `to_timeline`/`render` convert to the clock's unit while flattening for
  playback. That leaves `Playhead` and `TempoClock` untouched, which is where
  beats are correct — everything that runs on a tempo clock stays in beats:
  `Event.dur`/`delta`/`sustain`, a timeline's keys, patterns, routines, and the
  editor's ruler and snap.

  **What it reaches**: the alias and the doc lines here, the temporal metadata's
  meaning per body, the `Place` intent, the Python bridge (`to_document` /
  `from_document`), both clients' arrangement models and the host wherever it is
  told a length. The client half is written in `clients/python/PLAN.md` ("Three
  places call beats what is measured in seconds") with the concrete symbols; this
  entry is the format's half. **It is a fix and not a design**: the rule is
  stated, nothing here is waiting on a decision.

  **Fixed 2026-08-30, with the client half, in one commit.** `Beats` now names
  only what an onset is in (`Node::onset`, `Member::offset`, `Rules::quant`);
  `Seconds` names a length whose seconds were already fixed, and which of the
  two a `Node::duration` / `Member::dur` / `SegmentRef::duration` is in is
  **derived from the body** (`Body::duration_unit` → `TimeUnit`), so nothing new
  is stored and no writer can disagree with a reader. Three readers learned it:
  `Place` snaps an offset and leaves a length in seconds exactly as the hand
  gave it (a musical grid is not a ruler a recording was measured against),
  `resolve`'s `Mapping` carries `frames_per_second` beside `frames_per_beat`
  (the C ABI and the wasm request with it, `CORE_ABI_VERSION` 28), and
  `Body::relation` takes a tempo, since an aggregate can hold a lane of takes
  beside a lane of notes and their ends are no longer on one axis.
  `Member::length`/`duration_unit`/`end` are the accessors that keep the
  conversion in one place.

- ✅ **Material assembled from several windows had no body, so a joined clip was carried and not understood** *(found 2026-08-18 building the multitrack's join: the arrangement grew a `Segments` element — several windows onto several buffers, read as one — and wrote a `segments` node the crate could only keep as `Body::Unknown`)*. The forward-compatibility door did its job: the node round-tripped whole, so nothing was lost and the script-driven editor worked. What did not work is everything that *interprets* a document — the standalone host drew such a clip with no material, `Session::dangling` reported no missing source for its buffers, and a selection over it resolved to nothing.

  **`Body::Segments` closed it the same day**: a list of `SegmentRef` (which source, from which frame, for how long in beats), with the same `config` a `Vector` carries because what the element *is* is one thing to play. It is **not a sixth primitive** — it is the `Vector` primitive over more than one window, which is the framing the arrangement uses too. Four readers learned it: the source walk (`Session::dangling`), the resolver (one `Resolved` per window the selection reaches, since each is a different part of different material), the staleness check (the **stalest** of its sources, because an edit against it was made against every window it shows) and the standalone host's drawing (one clip, a take body per window, each over its own stretch — which the clip protocol had just made sayable).

  **What it leaves open**, recorded rather than resolved: a window is now expressible **two ways** — `SourceRef::range`, in frames, which only this crate's tests write, and the client's own (`start` on a segment, `start` in a buffer's config), which is what the Python writes because `to_document` is unit-less and cannot turn a length in beats into an end frame. Both are honest; two of them is one too many, and which survives is a question for whoever needs a frame-exact trim in the document (a host-side session editor is the first candidate).

- ✅ **Two placements of one element share a node id, and an edit on the second lands on the first** *(found 2026-08-14, dragging the second clip in `gui_daw.py` while writing the example; `composer.py` had the same shape)*. `to_document` stamps the id **on the element object** (`clients/python/clausters/form/document.py`, `_Ids.of`) so that a second conversion numbers the same node the same way - which is the property the whole history rests on - and the same object appearing at two offsets therefore serializes as two nodes with one number. Nothing rejects that document, and the two halves of one edit then disagree about which placement it named: the crate's lookup takes the **first** match (`intent::find_member_mut` returns the first member whose id matches), while `Editor._index` keys `node id -> (owner, handle, element)` and the **last** placement indexed overwrites the first. So a drag on the second clip moves the first placement in the document and writes the new offset into the second handle in the arrangement - two writes, two different placements, and on screen the clip the hand moved comes back to where it was.
  
  **Fixed 2026-08-17 by O14**: the id belongs to the member handle, so the two placements are two nodes and an intent naming one moves that one. The acceptance written below was run as a test — build the repeat, drag the second, and exactly that placement moves, in the document and in the arrangement.

  **What shipped first was the examples, not the fix**: `composer.py` (which absorbed `gui_daw.py` on 2026-08-17) now gives each placement its own element over the same server buffer, which is correct authoring and sidesteps the whole question. The model still accepts the ambiguous tree and still mis-addresses it, and it stays open because the fix *is* a decision - see "May one element be placed twice, and what does an intent name if it is?" under Open decisions, where the three answers and what each one costs are written out. Whichever wins, the acceptance is this case: build the repeat, drag the second one, and have exactly that placement move, in both the document and the arrangement.

- ✅ **A session does not round-trip: an aggregate loses its name, and a `Track` comes back as an `Aggregate`** *(found 2026-08-16, running the whole-loop example — `gui_daw.py` then, `composer.py` since the two merged on 2026-08-17 — and pressing open: the reopened piece draws an extra, empty-looking aggregate in the melody lane, and the lanes lose their labels)*. `to_session` -> `from_session` is the loop's fifth step, and what comes back is not what went in:

  - **The name is dropped.** `Aggregate(name="take")` and `Aggregate(name="melody")` come back anonymous, because `_body` (`clients/python/clausters/form/document.py`) writes no `name` and the document schema has no field for one. The multitrack labels its lanes from that name, so a reopened piece loses the labels it was authored with.
  - **A `Track` becomes an `Aggregate` of `Clang`s.** That is deliberate on the way *out* — the document has one `aggregate` kind, and a track is "an Aggregate with the restrictions of a multitrack view", which is what makes a note in a roll addressable — but nothing on the way *back* says the aggregate was a track. So one level of nesting appears that the author never wrote, and the editor draws it as an aggregate whose members are clangs rather than as a track.

  **Neither is a Python-side bug to patch there**: both need the document format to carry something it does not carry today (a node's name; what an aggregate *is* to a view), which is this crate, its schema and both clients. Recorded rather than folded into the example, and it is what the example's "open it again" step is really testing — *the same composition by identity*, which today holds for the ids and not for the shape.

  **What an eye pass measured, 2026-08-17** *(reopening `composer.py`'s four-lane piece)*: the take clips keep their waveform and the automation clip its curve, and **both roll bodies are gone** — the melody's because a `Track` comes back as an `Aggregate` of clangs (no aggregate is a roll any more), the pattern lane's because a frozen generator has no rendering to draw. The user reads it as "the piano roll is gone", which is the same defect at the size a real piece shows it.

  **The server already answers the naming half, and the document should copy it rather than invent one** *(pointed out by the user 2026-08-17: the server's groups carry a label too)*. `docs/schemas.md` states it as a rule worth reusing verbatim: **a name is a referenceable label, not a new identity** — the id remains what every command addresses and every reply reports, and the name is a second way to *refer* to the same group. A group is **born named** (the string rides in the creating message, so the label travels at the moment the writer knows what it is building), renaming is its own verb, an empty name clears it, and a refused label refuses the creation rather than leaving an anonymous group behind. And it buys navigation for free: every group contributes one path segment — **its name if it has one, its decimal id if it does not** — so `/mixer/drums` and `/1000/drums` both resolve and nothing is unreachable for being anonymous. That fallback is exactly what a document needs in order to grow names without making them mandatory or making them an identity, and both clients already speak it (`Group("mixer")`, `group.rename(...)`, `Server.group_at(...)`). The remaining half is the one the server has no opinion on: what an aggregate *is* to a view, which is this format's own question.

  **Acceptance:** the example's song survives `to_session` -> `from_session` structurally identical — same kinds, same nesting, same names — asserted by a test that compares the two trees rather than by an eye pass over a window.

  **Fixed 2026-08-17, and the format grew exactly two things.** A node carries an optional **`name`** — the server's rule verbatim, a referenceable label and never a second identity, so the id stays what an intent addresses and an anonymous node is reachable exactly as before. And `Body::Aggregate` carries a **`config`**, opaque like a leaf's, in which a writer puts the restrictions it had on that aggregate: the Python client writes `{"form": "track"}` and reads it back into a `Track`, and the crate is not one line wiser about what a track is. **There is still one aggregate kind**, which is the part worth defending — the tree does not act on view-ness, it carries it, the same door a generator's code goes through and the same treatment an unknown widget gets in the GUI protocol. The reasoning, including what it does not fix (two clients agreeing on a string no schema checks), is in `docs/decisions.md`.

  **What it cost across the packages:** the crate (two fields, and every construction site of `Body::Aggregate` in the workspace and the host), the Python bridge both ways, and the parity vectors re-run — which moved nothing here, the document generator using neither a named group nor a track, and instead turned up an unrelated stale vector that shipped on its own.

- ✅ **What a document calls a generator is `repr()`, so a saved session carries a memory address and nothing can find the algorithm again** *(found 2026-08-17, merging the whole-loop example into `composer.py`: its pattern lane comes back frozen and there is no key by which it could come back any other way)*. The conversion names what the document does not own with `_reference` (`clients/python/clausters/form/document.py`), which is the object's `name` when it has one and **`repr(obj)` when it has not** — so a `Pbind` lane serializes as `"<clausters.seq.pattern.Pbind object at 0x7f...>"`. Two things follow and both are bad. The file gets **content that changes between runs of the same script**, which is the one property O1's acceptance asked writing to keep (`deterministic`); and the reference is **unresolvable by construction** — `from_document`'s `resolve(kind, config)` is handed exactly that string, so a caller holding the very pattern cannot tell that this is the leaf it has. An `Automation` happens to work only because it carries a `name`.

  **It is the format's question and not the bridge's**, which is why it is here: a leaf is opaque by decision (the crate never interprets a generator's config), so what is missing is a **stable identity** for one — something an author sets, or the conversion derives and the format carries, such that reopening can hand the recipe back. Until it exists, "the same composition by identity" holds for the ids and not for what the ids name, and a client's only honest posture is the one the example now takes: resolve what can be named, draw the rest frozen. Related to the entry above rather than a duplicate of it: that one loses a node's *shape*, this one loses a leaf's *identity*.

  **Acceptance:** a composition with a pattern lane, saved and reopened by a script that still holds the pattern, plays that lane again — and the file it went through is byte-identical between two runs of the same script.

  **Fixed 2026-08-17, and the answer was the one the entry left implicit: the identity is a name an author sets, and `repr` is worse than nothing.** A reference now comes from three sources in order — the object's own name (a def, an `Automation`), the **element's** `name` (what an author writes for material that has none of its own, a `Pbind` being code), and nothing at all. An unnamed leaf is written with **no reference key**, so the file is the same bytes on every run and a resolver is never handed a string that cannot match; the leaf comes back frozen, which `form.render` already treats as structure. `Element` therefore takes `name=` throughout, and it is the same name the node carries and the multitrack labels its lane with — a label, not an identity, so two elements may share one, which is exactly what *the same algorithm used twice* looks like.

  **Both halves of the acceptance are tests rather than claims**: a named pattern lane saved and reopened by a script that still holds the pattern emits its events again (and the same file with no resolver emits none), and the same script run in **two processes** writes identical bytes — which is the only place the old defect was visible, since an address is stable inside one process. The example names its bass lane and resolves it, and the composition chapter grew the rule.

- ✅ **A note dragged on a generator's clip stays drawn where the hand put it, and the edit never happened** *(found 2026-08-14, reading `clients/python/clausters/gui/editor.py:471` while working out what an acknowledgement is for)*. The roll emits `"notes"`, `Editor._apply_notes` finds no editable timeline because the element is a forward-only generator, returns `False` - *"so the edit is a no-op"*, exactly as intended - and **sends nothing back**. The host has no way to learn that the edit was refused, so it keeps drawing the note in its new position until the next whole-tree redefine, which for a placement edit never comes. **Fixed by O3** *(2026-08-14)*: `Editor.apply` now answers every event it owns, and a refusal is the notes pushed back as they still are - so the host adopts the correction with the ordinary drop-and-adopt rule, no redefine and no second message. Two things the fix needed that the bug did not show. The editor must answer **only for widgets it drew**, or a poll loop shared with a second editor retires a pending edit nobody applied; and the answer must carry a **value**, since "refused" with nothing attached leaves the host guessing what to draw instead.

  It is the cleanest possible demonstration of the hole this crate closes, and worth keeping as one: nobody wrote a bug. The client is right to refuse, the host is right to draw what the hand did, and there simply was no message in the protocol capable of carrying "no". The fix is O3 and nothing else - the refusal is the previous value, pushed back and stamped - and this case is its acceptance test.

- ✅ **A selection's mapping states one tempo, so a selection cannot cross a
  tempo change** *(found 2026-09-01, taking the entry below — the one caller
  that legitimately kept a multiplication)*. `resolve::Mapping` carries
  `frames_per_beat` and `frames_per_second`, whose ratio is a tempo, and every
  beats↔frames conversion in `resolve.rs` uses it. That is correct for what a
  `Mapping` *says* — it declares a fixed relation between the two axes — and it
  quietly makes the surrounding operation wrong wherever the piece's tempo
  moves: a selection that starts before an accelerando and ends after it
  resolves to the wrong frames at one of its two edges.

  It is filed rather than fixed because it is a different shape of change from
  the one beside it. `Member::end` needed the *caller's* answer for one length;
  a `Mapping` is a value handed around and read many times, so replacing its
  ratio with a map means deciding what a `Mapping` is — a stated constant that
  a caller must not build across a tempo change, or a reference to the piece's
  map. The second is the same identity question the tempo value already waits
  on, so the two are read together.

  **Fixed** *(2026-10-07)*, and the question had been answered in the
  meantime: a tempo map is a **value** the caller holds (the clock's), which is
  what `Member::end` was already given through `SecsToBeats`. So a `Mapping`
  is neither of the two things above — not a constant, and not a reference to
  a map the document stores — but **the map itself, handed over for the
  call**: `Mapping { tempo: &TempoMap, frames_per_second, unit }`. The
  document still names no clock, and the identity question stays exactly where
  it was (a saved file saying which clock it is written against), because
  resolving a selection never needed it.

  Nothing in `resolve.rs` multiplies a length by a tempo any more:
  `Mapping::end_of` takes where a length starts, `frames_between` takes both
  beats and reads the seconds between them off the map, and a selection in
  frames finds its beats with `beats_at`. `length_in_beats`, `to_frames` and
  the private `tempo()` are gone. A caller whose tempo never moves builds a
  map of one segment and says so by doing it; both clients take a number for
  that (`tempo=1.0`, beats per second) beside a `TempoMap`.

  It crosses every binding in one change: `clausters_document_resolve` takes a
  tempo-map handle where it took `frames_per_beat` (`CORE_ABI_VERSION` 88),
  `JsDocument.resolve` takes the wasm `TempoMap`, and `Document.resolve` /
  `resolve_selection` take `tempo` in both clients. The parity vectors gained
  three selections under a map that doubles at beat 4 — in beats, the same
  stretch in frames, and one wholly after the change — and both clients land
  on the same spans.

  **Two more things in the same file read wrong, and were fixed with it.** A
  window of an assembled body (`Body::Segments`) opens at a **second** of its
  source — the format has said so since the unit was corrected on
  `SegmentRef` — and `resolve` was the one reader still taking `start` as
  frames, so a selection over a join resolved to the first frames of each take
  whatever the window. And the windows were all laid as seconds, when one onto
  a node measures beats (`SegmentSource::unit`): it contributes no samples
  either way, but it moved where the windows after it were found.

- ✅ **`TimeUnit::to_beats` takes a length and a scalar, and no position — and the format has nowhere to put a tempo** *(found 2026-08-31, auditing the call sites of the client's frozen-tempo bridge; `clients/python/PLAN.md` holds the other half)*. `TimeUnit::to_beats(length, tempo)` (`src/lib.rs`) converts a length in seconds to beats by multiplying by one number. Under a tempo that changes along the piece that operation is not imprecise, it is **undefined**: a beat is a logical coordinate, so the same stretch of seconds reaches a different beat depending on where it starts. Its caller `Node::end(tempo)` already has the onset in hand (`self.offset + self.duration_unit().to_beats(d, tempo)`) and simply does not pass it, so the local fix is to take the position — or to return the end beat rather than a "length in beats", which is the honest shape.

  **Fixed** *(2026-09-01)*: `TimeUnit::to_beats` is gone and the crate stops
  converting. `Member::end` and `Body::relation` take a `SecsToBeats` — a
  closure from `(onset, seconds)` to beats — so the caller, who holds the
  piece's map, answers the question the document cannot. A length already in
  beats never reaches it. `at_tempo(bps)` is the constant-tempo converter,
  written out so a caller that genuinely has one number **says so at the call
  site** instead of a scalar assuming it everywhere. A test places two
  identical takes at different beats under a piecewise tempo and reads two
  different ends, which is the defect made visible.

  The format still has nowhere to put a tempo, and that is no longer this
  entry's problem: a tempo map is a **value**, not a field, so what the format
  needs is an *identity* to reference one by — see "A tempo map is a value, and
  nothing said so" in `clients/python/PLAN.md`. Nothing here waits on it.

  One conversion stayed a multiplication and is worth naming: `resolve.rs`'s
  `Mapping::length_in_beats`. A `Mapping` states its own `frames_per_beat`, so
  its tempo is a constant *by construction* and the multiplication is correct
  for what that struct expresses. What it cannot express is a selection
  resolved across a tempo change — a wider question, filed below rather than
  assumed away.

  What it actually needs is the piece's **time map** (`clausters_core::tempomap::TempoMap`, which shipped 2026-08-31 with the client fix): the beat→second function, the integral of `1/tempo`. Both clients bind it, the clock holds one, and `Node::end` would take one instead of a scalar. That crosses the FFI, which is the same decision as the one below.

  **And the decision this waits on**: a piece with a tempo curve has nowhere to store it. The document carries no tempo field and `to_session` writes none, so a map lives only on the clock and is lost on save. The question is whether a tempo map is **notation** — part of the piece, saved with it, read by whatever plays it — or **execution**, which the clock builds and a save drops. The type is the same either way; what the answer settles is who constructs it and whether the format grows a field. Reading it against `O14` (what a leaf's config is *of*) is the natural place, since both are about what the document claims to carry.

- ✅ **A box is measured in the view's samples and filled with the source's
  frames, and nothing converts between them** *(found 2026-09-22, reading the
  drawing and drag path for the entry above)*. A box's `offset`, `dur` and
  `start` reach the host as samples at the **view's** rate (`clips` crosses each
  through `look.frame_at`, and `start` through `box_.start * look.rate`), while
  what bounds and fills it is counted in the **source's** frames: the host's
  `contents_of` answers the take's frame count, and the body is drawn frame for
  sample (`fit: false`). The two agree only when the source's samples were
  recorded at the rate the view measures in, and nothing anywhere checks that,
  converts for it or carries what would be needed to -- the props door takes one
  rate and a table of `{buffer, channels, frames}`, and `nodes::SourceInfo`, the
  plan's own table, carries `buffer` and `channels` and no rate at all. Measured:
  a one-second take of 44100 frames at 44.1 kHz, placed whole in a 48 kHz
  session, is drawn **48000 samples long over 44100 frames of samples** -- 8.8%
  of box with nothing in it, its edge stopping 3900 samples short of its own end.
  **It reaches sound, not only the picture**: `nodes::plan` reads the window's
  start as `window.start * sample_rate` with the session's rate, so the reader
  starts at the wrong frame of such a buffer by the same ratio.

  **`playrate` is the same question already stored**: a `Content` carries one
  (both clients write it, `Content.onto(..., playrate=)` / `Content.onto(w,
  {playrate})`), and outside constructors that pass `1.0` **nothing reads it** --
  not the picture, not the plan, not the drawing. A region at half rate reads
  twice the frames and would be drawn and bounded as though it read the same
  number.

  **The decision** is which one the document means, and it is one answer for the
  picture, the bound and the plan: either a source's samples are resampled to the
  session's rate when they are read in, so one sample is one frame by
  construction and `playrate` is the only ratio left; or a box's window carries
  the ratio (the source's rate and its `playrate`) all the way to the drawing and
  to `PlayBuf`, which means `SourceInfo`, the props table and `SourceWindow` all
  grow it. The second is also what a time stretch would need, and `SourceWindow`
  already reserves `fit` for the picture that produces.

  **Decided by the user 2026-09-22 and done: the reader resamples as it plays,
  and one ratio is read by everything that has to say it.** Nothing is converted
  on the way in -- a buffer keeps the rate its file was written at, as it always
  has and as scsynth does -- and the multitrack's reader crosses its phase by
  `BufRateScale(buf) * rate` and its window's start by `BufSampleRate(buf)`,
  which is `PlayBuf`'s own idiom spelled in the def that plays a box. So the
  **audio path asks a client for nothing**: what travels is the region's
  `playrate`, which is the half of the reading speed that is a decision, and
  `mixer::START` is now the source's **seconds**. The picture crosses by
  `SourceWindow::rate` and the edge a hand pulls by `Contents::rate`, both fed
  from the `rates` prop the projection writes (flat `box rate` pairs, a name
  list like `loops`) out of the source table both clients already keep, which
  grew a `rate` beside its `frames`. Zoomed to the sample a resampled box draws
  **the samples it plays** -- the reader's own linear reading between frames,
  through the shared `resample::Interpolator::played` that `BufRd` itself now
  calls -- with the band-limited reconstruction still drawn through them, so the
  resampling error is visible instead of being hidden by drawing one curve
  twice. The reasoning is `docs/decisions.md`, "A source at another rate is
  resampled as it plays, and one ratio says so everywhere". Held by tests on
  each layer: the reading in the core, the plan's seconds and playrate, the
  projection's `rates` and its window in the source's frames, the edge that
  stops at 48000 samples over 44100 frames, and the grid the dots are drawn on.
  **`playrate` is read now** -- it was stored by both clients and read by
  nothing.

  **And the join followed the same day**, since its refusal rested on the same
  mistake: a stitched buffer owns no samples, so joining takes at different
  rates converts nothing. Each part carries the ratio its source makes with the
  join and is read through it (`dsp::stitch`), a part at the join's own rate
  keeps its indexed load behind a comparison made at build, and what the command
  states is how many frames **of the join** a part fills -- the ratio itself
  never travels, because the server takes it off the buffers. `segments` gained
  the ratio per span so the picture says the same thing, and the projection
  counts a part's contribution rather than its span. A join is still not a
  *converter*: nothing is written out at its rate, so a mixed-rate join costs
  the reading and not memory. `docs/decisions.md` carries both halves.

- ✅ **The takes a history can still reach are never given back** *(found
  2026-09-22, settling where a join's samples live)*. An undo frees no take: the
  reconciliation frees only the curve tables, and a source the multitrack stops
  naming is still a source -- deliberately, since that is what lets a redo find
  what it left rather than build it again (`clients/python/PLAN.md`, "A second
  join left an empty box, because a source the piece stopped naming is still a
  source"). So a session that joins, undoes, joins again holds every join it
  ever made, and the takes under them, for as long as the history can name them.
  It is the right default -- rebuilding a join on a redo would cost the stitch
  and, for a mixed-rate one, would have to be identical rather than equivalent
  -- and it has no ceiling.
  **What is undecided** is who gives them back and when. Discarding history is
  the event that makes a source unreachable, and nothing today connects the two:
  the pile is trimmed (`clausters-editing`'s history has a depth), the sources
  are not. The shapes worth weighing: a source table that walks the pile and
  frees what no entry can reach any more; a lifetime tier below `temporary`
  whose buffers die with the step that made them; or leaving it and saying so
  in the docs, which is honest for a session and wrong for a long editing day.
  **Related:** `Lifetime`, whose `temporary` tier already says "dies with the
  edit session unless a save promotes it" -- nothing enforces that either.
  **Decided 2026-09-22, and the work is `X1`'s** (`crates/clausters-apps/PLAN.md`,
  "The design: the history holds references to immutable takes, never
  samples"): the first shape, generalized -- a source is freed when no root
  reaches it (the document, the history's entries, the clipboard), with a byte
  budget beside the entry count and an explicit clear. This entry closes when
  that lands. **Closed 2026-09-22 with `X1`**: an entry names the sources
  each half reaches (`history::Entry::holding`), the history reports the ones
  none reaches any more (`History::released_sources`) and trims by bytes
  (`History::trim_to_bytes`); the audio editor's context frees them and spills
  the oldest to disk past a resident budget. What is left for the multitrack
  is the entry below.

- ✅ **The multitrack's joins do not say which takes they hold** *(found
  2026-09-22, closing the entry above)*. The history can now hold and give
  back takes (`Entry::holding`, `History::released_sources`), and the audio
  editor uses it. A multitrack's entries go through the document's log and
  name no sources, so a join minted and undone is still kept for as long as
  the session runs. The mechanism exists; what is missing is the multitrack's
  entries stating the sources their two halves reach, and a caller (the
  clients, the standalone host) freeing what comes back. **Fixed
  2026-10-01**: a multitrack leg holds the joins its two halves name
  (`MultitrackIntent::sources`), the editing context hands a released one
  back under the multitrack's member (`Freed::sources`) once no region reads
  it, and both clients and the standalone host free its buffer and drop it
  from their table.

- ✅ **A fade's curve is editable only if the curve drawn is the curve heard**
  *(found 2026-10-08 by the user)*. A box's `fade:` trapezoid lets a hand bend
  each sloped side: `points::bend_curve` sets the side's shape to `curve`
  with a curvature, `picture::read_fades` keeps both on the region's `Fade`,
  and the reader plays them through `EnvShape` -- so on a region's own edges
  the drawn curve should be the heard one (read from the code, not yet checked
  by ear). Where they part is a **join**: a seam takes each box's fade
  *length* into its part (`Part::fade_in`/`fade_out`) and `/buffer_stitch`
  plays every part's fade as a straight ramp, so a curve drawn on an edge
  before a join is played linear after it, and so is the default equal-power
  one. The rule, from the user: if one shape is kept (equal power, `wel`),
  the picture must not let a hand bend it; if not, the sound has to follow
  what is drawn. **The first implementation is the easiest of the two**, and
  that choice is made by measuring both: taking the bend off the trapezoid
  is a host change, and a shaped seam is a change to `/buffer_stitch` (a
  shape and a curvature per part's fade), to its builders in both clients
  and to `/buffer_parts`.

  **Closed 2026-10-08, the same day, by the sound following the picture.**
  Measured, taking the bend off was not the smaller fix: the default
  equal-power edge also played linear at a seam, and a script can state any
  shape on a `Fade` in both clients, so keeping one shape would have meant
  forbidding it there too. A seam now has a shape: `/buffer_stitch` and
  `/buffer_parts` carry `fadeInShape fadeInCurve fadeOutShape fadeOutCurve`
  per part, a join's `Part` keeps them (`SeamShape`), and the server plays a
  part's fade through the same `shape_value` the reader and the picture use.

- ✅ **The crossfade switch acts on the edits that follow, and leaves the
  overlaps already made as they are** *(stated 2026-10-08 by the user)*.
  Today `TakeLane::edges` works the crossfade out whenever the multitrack is
  read, from `Defaults::crossfade` and the overlaps as they stand, so
  turning the switch off takes the crossfade away from every overlap already
  made, and turning it on gives it back to all of them. What the switch is
  meant to be is a facility of editing: an edit that makes an overlap while
  it is on writes the crossfade into the two regions' own fades, and turning
  it off changes no region. That moves the rule from reading to editing --
  the edit that moves or trims a box stretches its neighbours' fades and its
  own, and `TakeLane::edges` goes back to reading each region's fades as
  they are. Settled with it: what a later move does to a fade a crossfade
  wrote (a box dragged out of the overlap keeps the long fade, or gets its
  own back), and whether one gesture that writes a crossfade is still one
  entry in the history.

  **Closed 2026-10-08, the same day.** A hand's edit (`picture::read`) asks
  `TakeLane::crossfaded` and writes the crossfade as `FadeRegion` edits in
  the same report, so the move and its crossfade are one entry and one undo;
  `TakeLane::edges` reads the fades as they are. Settled: a fade the
  crossfade wrote is marked (`Fade::crossfade`), follows its overlap when a
  later edit moves it, and goes when the overlap does, giving the region its
  own edge back; a fade a hand or a script states is never written over; an
  overlap no edit touched is left as it is; and a script's edits state the
  fades they want -- the switch is the hand's facility.

- ✅ **An id can still be written by hand, so nothing keeps it unique**
  *(found 2026-11-10 by the user: editing a pedal curve in a roll over an
  MPE file read back, a click on a point added another at the same time, the
  points could not be dragged, and the host panicked asking for a 384 MiB
  vertex buffer)*. A sequence's events, its curves and its notes' curves draw
  on one counter (`EventSequence::hold`, `mint`), and the roll names a curve
  by that id, so two curves sharing one are drawn as one and every edit
  writes each one's points into both. `realize` gave the curves it derives
  from a page an id counted from the sequence's own curves alone; that one
  is fixed (`events::realize`, it now keeps the replaced lane's id or
  mints). What is open is that the counter is only a convention:
  `Automation::id` and `Event::id` are public fields, and
  `events::conversion` still numbers the curves it carries over the same way
  (`next` from the page's curves, `conversion.rs`). **To do:**
  - **One allocator per document, and the only door.** An id is obtained
    from the structure that owns the counter, never computed; the fields stop
    being assignable from outside the crate, and the places that build a
    curve or an event (conversion, realize, scopes, split, the file readers)
    ask for one. No check runs when an id is added, because none can be
    written that the counter did not give.
  - **The host mints none.** It is a view: it draws with the ids its owner
    sends and reports a new curve or note without one, for the owner to
    mint. Two allocators would need a split space, as `--id-share` gives the
    host against the audio server; one makes that unnecessary. Checked
    2026-11-10: both clients already delegate (the sequence writes `id: 0`
    and the core mints; the multitrack's objects call its `mint`), and the
    host mints no document id; the curve emitter's ids are playback's own.
  - **Validation only where a document comes in, and it repairs.** A repeated
    id can only arrive from outside: a file saved before the fix, a MIDI
    file, a sequence a client assembles. `hold` already runs there; it
    renumbers a repeated id, keeping the first and minting for the rest. For
    a curve that is safe, since only a view names it. For an event it is
    not silent: `joins`, a layer's note and a selection name events by id,
    and with two sharing one the repair picks one reading, so it repairs and
    says so.

  **Closed 2026-11-10.** Reviewed every door ids come in by, and every
  minting. The rule taken is the one `docs/decisions.md` records, "An id from
  outside is taken in, never trusted": a sequence repairs what it is given
  whole and checks one event or curve alone (`events::ids`), the events
  choosing before the curves; a multitrack is repaired when read and refuses
  an intent whose ids name something else (`multitrack::ids`); a symbolic
  score repairs repeated item ids before each operation and after reading a
  document; the general tree refuses a members list that would repeat an id.
  The fields stay public, since `Automation` is shared by four holders and
  privacy would not cover a whole sequence restored or read anyway: the doors
  are what guarantee it. Found on the way and fixed: `events::conversion`
  numbered by hand; `writeback` incremented the counter by hand; the host's
  cut of a note copied its id, so the owner read two notes under one; a
  multitrack split cloned the region's curves into the second half under the
  same ids; one of the multitrack's two minting functions left the markers
  out; and a tree `SetMembers` was not checked at all.

- ✅ **Writing a sequence as notation renumbers its events from the page**
  *(found 2026-11-10 reviewing the ids)*. `ToNotation` replaces the sequence
  with the page's render, whose events are numbered from 1 again
  (`events::score`), so an event id a client holds from before the
  conversion may name a different event after it. Nothing repeats (the
  result is checked as a whole), but a stale reference is not stale on its
  face. **To do:** number the render's events from the sequence's counter, or
  carry each event's id where the page still has it, with the notation
  section's references (`spanners`, `controls`, `marks`, `items`, `joins`)
  moved with them.

  **Fixed 2026-11-10, the same day.** A note of the page that is a note of
  the performance keeps that note's id, a curve the id of the curve it is
  (the note's curve on the same target, the sequence's lane on the same
  channel), a curve the page does not write keeps its own, and what the page
  adds is minted from the sequence's counter, with every reference to an event
  (`joins`, and the notation section's `items`, `spanners`, `controls` and
  `marks`) renamed with it (`conversion::number_as_the_sequence`,
  `EventSequence::rename_events`). Found with it and fixed: the conversion
  dropped the performance's own note curves, a bend among them, which a page
  does not write; they are kept on the note where the page writes nothing on
  their target.

- ✅ **A multitrack split leaves the second half's curves where they were**
  *(found 2026-11-10 reviewing the ids)*. A region's curve is measured from
  the region's start, and `SplitRegion` gives the second half a copy of the
  region's curves unshifted, so they read `at - position` late; a note's
  split cuts its curves at the point (`events::split::cut`). **To do:** cut
  a region's curves at the split the way a note's are, through the same
  function made generic over the axis, rather than a second one.

  **Fixed 2026-11-10, the same day.** One cut for both
  (`multitrack::nodes::cut_points`, generic over the axis): a note's split
  and `SplitRegion` cut each curve at the seam, each half measured from its
  own start, and neither jumps, a curve entirely on one side included (a
  note's split used to drop it from the other half, which then jumped). A cut
  made in a window was the larger half of this: it arrives as a report of
  boxes, and the second half was made as a bare window onto the source,
  losing the region's curves, fade out, layer and mute. It is now read as the
  region it was cut from (`picture::cut_half`), and the first half keeps its
  side of each curve.
