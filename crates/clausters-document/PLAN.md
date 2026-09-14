# clausters-document - implementation plan

*Opened 2026-08-14, out of the discussion that began at the GUI host's D and H tracks and ended somewhere else: those two tracks were trying to give the host the functions of an owner, and the owner did not exist anywhere a host could reach. This crate is that owner. The `Ox` labels are this file's coordinates and appear nowhere else.*

## What the crate is

**The document.** The single authoritative model of a composition: where each element sits in time, how elements group, what a selection is, what an edit means, and what an edit's inverse is. It is the layer `clausters.form` is today in Python, moved down so that every deployment mode binds the same one instead of re-deriving it.

**The crate and the Python module do not share a name, and the difference is precise rather than a loose end.** `clausters-document` is the document: the tree, the edit semantics, the log, the format. `clausters.form` is the **arrangement API over it** - how a composition is written in Python, which is ergonomics rather than model, and which is why it keeps the name it has (the web client gets the same surface under the same name). Naming the crate `clausters-form` was considered and dropped: it would have made the binding one-to-one at the cost of a name that describes less than what is inside - the log, the clipboard, the selection and the session format are not "the arrangement" - and it would have named the crate after the module that predates the insight instead of after the thing the four layers below actually establish. The prose rule is unchanged either way: the layer is *the arrangement*, its view the *multitrack editor*, and no document ever calls it "the form".

## Why it exists, and why now

Three deployment modes share every surface here: **client + host + server** (Python or TypeScript driving a GUI host against an audio server), **host + server** (the `standalone` editor, no language anywhere), and the headless client + server with no host at all. The document is the one thing all three need and none of them can borrow from another.

**The forcing argument is `standalone`, not parity.** A host that edits needs a document; today the document exists only in Python. So either the standalone host grows a second implementation of it - the two-owner problem in its worst form, across two languages - or the document moves to a crate every mode can link. There is no third option, and the cross-client rule the project already states ("keep all language-agnostic logic in the shared core and push it down there *as you write it*") says which one.

Parity is the second argument and it is ordinary: the web client would otherwise re-derive the same model from the same roadmap, and the two would drift exactly where drift is most expensive.

## The four layers

The load-bearing decision, and the one every other decision here follows from. Editing spans four layers, not three parts, and confusing them is what makes an editable view look like it breaks MVC:

| Layer | What it is | Where it lives | Mutates | Persisted | Undoable |
|---|---|---|---|---|---|
| **Sources** | the material itself | files on disk, and the server buffers they are loaded into | **no** - never overwritten, whatever their lifetime | by lifetime (see below) | no (an edit makes a new one) |
| **The document** | the description of what is played and when | **this crate** | yes | yes | **yes - the log is here** |
| **Presentation** | what is derived to be looked at | the GUI host | derived | cached | no |
| **Screen state** | zoom, scroll, selection in flight, a drag | the GUI host | yes | sometimes | no |

The rules that fall out: **an edit never writes a source** (it writes the document); **presentation is invalidated, never synchronized** (it is derivable from sources plus document); **screen state never enters the document and never enters the log**; and **the document is the only thing that is "the model"** - calling anything else that is what disfigures the pattern.

This is the pattern every audio editor already uses and none of them writes down as such: a DAW's regions reference source files nobody rewrites (Ardour's playlist/region model), and a destructive sample editor makes the source cheap to replace rather than making the edit destructive in place (Audacity's reference-counted block files). `/buffer_setRange` keeping the buffer's shape, and failing rather than clamping past the end, is that rule already stated on our own wire.

**"An edit never writes a source" needs one distinction to stay true, because we build both editors in one.** A **non-destructive** edit - a placement, a trim, a fade, a grouping - changes the document and no material at all; that is the composition, and it is what the document persists. A **destructive** edit changes samples, and it does not become an exception to the rule: it writes a **temporary** source of its own and never the one it started from. The next section states that as a decision; the layer table's "never overwritten" is what it protects.

## What the crate owns, and what it does not

| In the crate | Not in the crate |
|---|---|
| the tree: elements, placements, groups and their two kinds | what a `Pbind` is, how a def is compiled, how anything is played |
| the intent vocabulary, and the only implementation that applies one | the gesture that produces an intent (the host), the algorithm that consumes a range (the client) |
| the outcome of applying an intent - which is the acknowledgement's content | the wire itself (OSC framing, `/gui_*`) |
| the log: entries, inversion, cursor, coalescing, budget | the pending overlay and its drawing |
| the typed selection and the typed clipboard | the marquee, the lasso gesture, the spectral hit geometry |
| the session document and its provenance | the interpreter that could re-evaluate a generator |
| the version, and staleness detection | the transport that carries it |

**The dependency direction is one-way and absolute: the document never knows a widget.** What joins the two sides is a third thing - the **intent vocabulary** - which both depend on and neither owns. The host depends on the crate for the intents alone (an enum, not a model); the clients depend on all of it. A document that knows what a widget is has put the view back inside the model, which is the whole thing this crate exists to undo.

**The crate is not on the RT path.** The server does not edit - it stores sources - so it is not a consumer, and the crate is free to allocate, to use `std` and to be ordinary Rust. None of `clausters-core`'s constraints apply here, and that is worth stating because the crate sits beside it in `crates/`.

## Decisions taken before opening the track

- **A destructive edit writes a temporary source, and becomes material by being rendered.** *(Settled 2026-08-14. The question was whether a sample edit produces a new source or mutates a session-owned one; the answer separates the two editors instead of choosing between them.)* The **composition is the persistent artifact** - descriptions, in the document, in the folder where the session is explicitly saved - and a **destructive edit is a session over one element's material**, in scratch: opening an element for sample editing makes a temporary working copy, the strokes go in place on *that*, and the composition takes the result only when the edit is **confirmed**, at which point the scratch is materialized as a file the document repoints to, with its provenance. Discarding deletes the scratch and the element still names what it always named. Three things make this the shape rather than a workflow preference. It is **the layer's own change of state** - a generator element becoming a generated one, by the verb the layer already has (**render**), which is also what a bounce already does - so it introduces no machinery. It makes immutability **strict instead of almost**: no file is ever overwritten, not the user's and not the session's, so the rule needs no exception written beside it. And it puts copy-on-write at the granularity that pays for itself, **the editing session** - one copy for a hundred strokes - rather than per block, which would rewrite a megabyte for a pencil stroke of fifty samples and would have to be flattened back into a buffer to be heard, since the engine plays buffers and not sequences. In-place is also where the real-time question is already answered and not by us: `/buffer_setRange` lays a write into a copy that replaces the buffer whole, so the engine never reads a torn one. **The property to state rather than hide**: while an edit session is open, a synth reading that material hears each stroke immediately. In a sample editor that is the point; it is worth naming because in a transport-driven view it can surprise.
- **A source has a lifetime, and the document records it.** *External* (the user's file - read-only, never touched), *session* (persisted beside the document), *temporary* (scratch, dies with the edit session). It is a small field and it is what makes **saving honest**: without it a save in the middle of a destructive edit writes a reference to a file that is about to be deleted, and with it a save knows what it has to promote.
- **The version is two counters, not one.** *(Settled 2026-08-14.)* The **document** carries a monotonic version, and each **source** carries its own generation. They answer different questions and one number cannot do both: a destructive stroke changes a source's *content* while its identity stays put, which a document version cannot express, and a clip drag changes the document while every source is untouched, which a source counter cannot. With the pair, each reader invalidates only what actually moved - the waveform re-reads a span, the arrangement view does not redraw at all - and an intent naming both is stale against whichever one it was actually made against.
- **The stamp carries the versions from the first message, not from a second pass.** *(Settled 2026-08-14.)* `/gui_ack` ships with `seq` **and** the versions together in O3, rather than `seq` first and staleness later. The reason is arithmetic rather than principle: the message has four ends - the host, the Python builder, the TypeScript builder and `docs/gui-protocol.md` - and a two-counter version would mean touching all four twice, which costs more than carrying the field before the milestone that reads it. O4 then implements what the stamp already carries.
- **The stamp ends in an optional reason.** *(Settled 2026-08-14.)* A trailing string the owner may send and the host may ignore. It changes no mechanism - reconciliation stays the one rule, and a refusal is still the previous value pushed back - but without it a refusal is silent by construction, and a note that springs back with no explanation teaches *sometimes it does not work* rather than *not here*. It is optional because most acknowledgements have nothing to say, and it is in from the start because reserving the place costs an argument and appending it later costs the four ends again.
- **One gesture is one intent.** *(Settled 2026-08-14.)* The owner hears about an edit when the hand lets go, not while it moves: a drag draws its own pending for its whole duration - which is what the pending overlay is for - and emits once on release. So the log's entry, the wire's traffic and the acknowledgement's stamp are one to one, and none of the three has to coalesce anything. The alternative is not wrong so much as unpaid-for: streaming intermediate values costs a round trip per motion event to produce a state the owner discards on the next one, and its one real gain - something *outside* the editor following the gesture live, a synth sounding the buffer as it is drawn - is a feature nothing has asked for. If it ever is asked for, it arrives as a declared streaming mode on top of this, not as a change to what an intent means.
- **The clipboard stops being a `String`.** *(Settled 2026-08-14.)* K6 made it one and the notes block still travels that way, but a typed clipboard that can hold samples cannot: base64 inside JSON is the re-encode the project's own bulk rule exists to forbid, at 4/3 the memory, and an audio clipboard is the large case by definition. So the clipboard becomes a typed document - a `kind` plus its structure - with bulk payloads carried **beside** it as the existing little-endian `f32` blobs rather than inside it. One mechanism still, one carrier per kind of content: exactly the split `/buffer_setRange` and `/buffer_set` already draw between a payload that scales with the audio and one that scales with the parameters.
- **While an edit session is open, the buffer leads and the file is written on confirmation.** *(Settled 2026-08-14.)* The scratch has two representations because two consumers need different things - the host maps files, the engine plays buffers - and the one that leads is the one the work happens through: strokes write the buffer with `/buffer_setRange`, that is what sounds, and `/buffer_export` writes the file when the edit is confirmed. The host does **not** remap per stroke: it already holds what it drew and the acknowledgement confirms or corrects it, so it re-reads the file only on reopen. The alternative - the file leads and the buffer is a projection reloaded per edit - is what a headless or NRT edit would reach for, and it is not needed for one: such an edit writes the file and loads it once, without the file having to be the authority. The case this design is for is editing while listening, and reloading a whole buffer per stroke is the wrong end of it.
- **A save in the middle of a destructive edit promotes the scratch and leaves the edit open.** *(Settled 2026-08-14.)* The working copy changes lifetime - temporary becomes session - the document keeps naming it, and the log is untouched. The two alternatives both make saving mean something it should not: auto-confirming turns a save into an edit, and refusing until the edit is confirmed or discarded makes the safest habit in the program the one that is blocked. This is the same rule H already fixed for undo (**a save is not an undo boundary**), applied to material instead of to history. What it costs is stated so the format is designed for it rather than surprised by it: a saved session can carry material whose edit is still open, so the document must be able to **express and reopen** that state - the source's lifetime, its provenance, and the fact that confirmation has not happened.
- **The server resynthesizes, and every other audio processing too.** *(Settled 2026-08-14.)* A spectral edit leaves as the **parameters of a spectral operation the server runs**; the host draws it and the document records it, and neither of them performs DSP. **New UGens are written where the operation does not exist yet** - that is the ordinary cost of the rule, not a reason to bend it. The two rejected answers are worth naming because both look reasonable in isolation: the host doing the weighted overlap-add itself already has the complex analysis for drawing, and would only need the inverse - but that puts audio processing inside a window, mixes the domains, and duplicates a chain the server already owns (`FFT`/`PV_*`/`IFFT`); and the *client* resynthesizing would demand the identical analysis on both sides (same transform, window, hop and complex frames) and would still be a second processor. One place performs audio processing and it is the server. The host computes only what plotting needs, which is the rule `gui-widgets` already states and this decision keeps rather than qualifies.
- **A standalone host opens a generator element frozen, and frozen stays the fallback if it ever re-evaluates.** *(Settled 2026-08-14.)* With no language attached, a generator leaf shows its **last rendered result, read-only** - which is what the document already holds as concrete material, so nothing has to be interpreted and the opaque blob stays opaque. That matches the posture standalone already has (a bundle is a GuiDef plus GraphDefs: declarative artifacts, not scripts). Re-evaluation is a **later capability, not a later replacement**: a host that gains an interpreter re-evaluates what it can resolve and **falls back to frozen for everything else** - an unknown generator kind, a missing script, a language it does not embed - which is the same rule the widget protocol already runs on, where what cannot be interpreted is laid out rather than dropped. So frozen is not the version before the good one; it is the floor the good one stands on, and both states have to remain expressible in the document for either to work.
- **A leaf is opaque, and that is forced rather than chosen.** The document holds a leaf as an **id, a kind and a configuration blob it never interprets** - never the material and never the algorithm. This is not an FFI convenience: a generator *is code*, in the language of whoever wrote it, so no crate in any language can own one. Two earlier decisions had already reached it from other directions and can now be read as the same one - H1's ("a generator is code: going back is rewriting the structure and evaluating it again") and C38's ("an algorithm is never serialized, the way a project file never serializes a plugin"). It is also the four-layer split again: **the sources live elsewhere** (buffers in the server, defs and patterns in the client), and a document is a description.
- **The clients round-trip; they do not hold handles.** The crate is the **normative model and the format**, and each client keeps its idiomatic surface over it, rather than every client holding pointers into a Rust object graph across a C ABI. What makes this safe rather than a return to the parity problem is one rule, and it is the crate's central discipline: **the crate is the only thing that applies an intent.** A client does not apply and then report - it hands over the document and the intent and receives the new document plus the outcome. One implementation of the edit semantics, in one language.
- **One tree, not two.** What makes the round trip cheap is that the client's document *is* the crate's document, with idiomatic accessors on top - not a parallel structure that synchronizes with it. That is JUCE's `ValueTree` shape (one generic tree carrying its own undo, with typed views over it), and it preserves the property that makes `clausters.form` good today: it stays "a thin adornment", now over the crate's tree instead of over loose objects.
- **Every intent is absolute, never relative.** An intent states the resulting value (`"note" id pitch`), never the increment (`"transpose" id steps`). Two things follow and both are load-bearing: replay becomes unnecessary - a pending edit simply stays drawn over whatever authoritative state arrives, with nothing to recompute - and an intent becomes **idempotent**, which matters on a UDP leg where a resend must be harmless. The reason this is a decision and not an accident is that the alternative is worse than it looks: relative intents need to be rebased against a corrected state, and rebasing them is exactly the netcode replay that would require the host to hold an executable copy of the document. *"In the owner's terms"* - the established rule that an intent speaks diatonic steps rather than pixel deltas - is a rule about **units**, not about deltas; `"note" id pitch` is as much in the owner's terms and needs no rebase.
- **The acknowledgement is the authoritative state push, stamped - not a second message.** The owner answers an intent by pushing the state that now holds, carrying the sequence number of the last intent it processed. That collapses three outcomes into one shape: **applied verbatim** (the value equals what the host drew), **applied transformed** (the value is the effective one, post-snap or post-clamp), and **refused** (the value is the previous one, unchanged). The host needs no branch for them - its whole rule is *drop every pending whose `seq` is at or below the stamp, and adopt what arrived* - and a refusal needs no error path, because "the state after your gesture is the state you already had" is a state push like any other. It is Ardour's `rdiff()` seen over a wire: **what reports the change is the model, never the hand.**
- **The stamp is a verb, not a property.** It travels as `/gui_ack <seq> [<version>]`, in the same bundle as the value pushes and after them, and it is sent **always** - including when nothing changed, since that is exactly what a refusal is. It is not a widget property because it is scoped to the *conversation* rather than to the tree (`seq` is per client, and two clients driving one window would collide on a single prop), because it does not round-trip and so cannot honor K5, and because the property channel drags along machinery it has no business in - `/gui_query` reporting, GuiDef persistence, the props parity test. The project's rule against new `/gui_*` addresses exists to stop a *widget or a prop* from becoming an address, and its stated escape hatch is a new payload on `/gui_event` - which is the host-to-client direction. This is the client-to-host direction, and it is the reply `/gui_event` never had: every other thing the host asks (`/gui_query`) has one.
- **Consistency by duplication is what this replaces.** Today the client ships its rule down to the view - the lane's `snap` grid travels in the GuiDef and the same snap is implemented on both sides - and the round trip closes because both sides did the same arithmetic. That works for a grid and does not generalize: a bus allocation, a normalize, a user-written function cannot be shipped down. Recording it because it is the honest reason the acknowledgement never seemed necessary, and the reason the places where it already fails are so quiet.

## Open decisions

The eight that opened with this file were taken one at a time and are in the section above, each with what it rejected. The heading says what belongs in it: a question whose answer changes what gets built, kept out of the milestone that would otherwise settle it by accident. It is not a backlog - every `Ox` below is that - and it is not "Future directions", which is a capability nobody has built rather than a choice nobody has made.

- ✅ **May one element be placed twice, and what does an intent name if it is?** *(opened 2026-08-14 by the defect at the foot of this file, which cannot be fixed without it)*. `Group([(0, take), (4, take)])` is the obvious way to write *this take, twice*, and today it produces two document nodes carrying one id - so an intent that names that id names both placements and reaches whichever the lookup finds first. The question is not how to break the tie; it is **what an id identifies**, and the three answers put it in three different places. **Forbid it**: an element is in the tree once, `to_document` raises where it is not, and writing the repeat is the author's job (a second element over the same server buffer - which is what the examples now do). Cheapest, and it makes the arrangement's most natural sentence illegal. **Copy it**: the conversion clones a second appearance into its own node, so the id stays an identity and the tree stays writable as it reads. Preserves the sentence, and quietly decides that two placements of one take stop being one thing the moment they are edited - which is the wrong answer for the case the sentence is usually written *for*, an element repeated so that editing it edits every repeat. **Name the placement**: a placement intent addresses the member - the (owner, index) it moves - rather than the node it moves, which is what a placement already *is* here (`find_member`, and `Editor._index` already keys on the owning group plus the handle for exactly this reason). Correct, and it is the one that moves the intent vocabulary, so it is the one that must not be settled by accident inside a milestone that only needed a tie broken. **What each costs is asymmetric across the packages**: the first two are the Python bridge alone, the third is the vocabulary, both clients and the host.

  **A second case widened this on 2026-08-17, and it is what makes "what an id identifies" the live half of the question**: two *different* elements can hold one id, with nothing shared between them. Node ids are minted per conversion and stamped on the element object, starting at 1 for every root, so two compositions in one script both number 1, 2, 3 and material reused across them collides — recorded with its measurement in `clients/python/PLAN.md`. It is the same failure downstream (the lookup takes the first, the editor's index keeps the last) and it is **not** avoidable by authoring, which the placed-twice case is. So whatever this decision picks, it also has to say **who owns uniqueness within a document** — the bridge that converts, or the crate that parses and applies. Enforcing it here is what makes it hold for the host and for a client that does not exist yet, and it is the cheaper half of "name the placement" rather than a separate build.

  **Half of that shipped 2026-08-17 and it narrows this question rather than answering it.** Uniqueness is now owned by **both** ends, each for what only it can do: the Python bridge cannot mint a collision any more, and the crate refuses one on deserialization (`Document::duplicate_id`). But the check is deliberately drawn so that it does **not** touch this decision — a repeated id whose nodes are *identical* is carried, because that is one element placed twice and refusing it would pick the "forbid" answer from inside a check about a different failure. So what is left here is exactly the original question and nothing else: not whether a document may be incoherent (it may not), but what an id **identifies** when the document is ambiguous and consistent.

  **The answer, argued by the user 2026-08-17 and recorded here because it stops the three from being equivalent.** The three answers were written as a choice with a cost each. They are not: read against what a multitrack *is*, only one of them survives.

  - **A clip is a window onto material, and the identity is the material.** That is the whole of non-destructive editing, and it is what every multitrack editor already does: several clips refer to one file, or to parts of one file, and the file is not touched. So an element appearing at two offsets is the **ordinary** case, not an error to forbid and not something to silently copy — and *forbid* and *copy* are exactly the two answers that make it one.
  - **Material comes in two regimes, and the document cannot currently say which.** An **instance** — a file, a break-point curve, a MIDI/OSC sequence — is a thing: two clips that copy it are two instances and may diverge by editing. A **function** — a `Pbind`, a routine, a def — is an algorithm: a clip *evaluates* it, two clips are two evaluations, **possibly with different arguments**, and evaluating the same function in two places is as unremarkable as calling it twice. The analogy is direct and it is the axis the format is missing: today a leaf is opaque and nothing says whether referring to it twice means *one thing seen twice* or *one recipe run twice*.
  - **So the answer is "name the placement", plus that typing.** A placement gets its own identity and carries a **reference** to what it places (and, for a function, the arguments of *that* evaluation). The node stops doing two jobs at once — being the material's identity and being one appearance of it — which is precisely the conflation that made a repeated id ambiguous in the first place. A `Place` naming a node is what makes an edit survive its siblings moving (this crate's own reason for it), and a member handle is what makes it survive the *same element* appearing elsewhere.

  **What this leaves to build, which is why it stays an open decision until the shape exists rather than closing as an answer:** a stable identity for a member (the document has none today), the instance/function distinction on a leaf, where an evaluation's arguments live, and what a *copy* is as an explicit act — copying an instance forks it, copying a function's placement does not. It is the vocabulary, both clients and the host, and it is a one-tree question: a member has no identity to name while the client keeps a parallel tree and re-derives the document from it.

  **Answered 2026-09-06 by "The turn: the arrangement stops being a projection"
  below, and answered with the option this entry had already argued was the only
  survivor.** A **region** is the placement with a name: its own identity,
  separate from the source's, so an intent naming a region names one appearance
  and a source referenced from six places is referenced, not copied. The
  instance/function typing this entry asked for lands on the **source**, and a
  region carries the arguments of its own evaluation. What kept it open was that
  the shape did not exist; O21 is that shape, and the entry closes into it rather
  than being restated there.

  **The visual layer is already on the right side of this and is not what needs fixing.** A widget has its own id and the editor maps widget id -> node, so the picture is independent of the model by construction. What is missing is one level down: the *placement* has no identity in the document, so the node plays both parts.

  **Settled 2026-08-17 by O14**, which is the shape this decision was waiting for: the id moved to the member handle, so each placement is its own node and an intent naming one is unambiguous — and what may be placed twice is what the node *references*. The crate needed no change at all.

  **Against the check that shipped:** `duplicate_id` compares nodes *by value*, which accepts this case correctly and for a weaker reason than the one above — two windows onto one material serialize identically, so it passes, but the model still cannot tell "two views of one instance" from "two objects that happen to be equal". That is not a defect in the check (it refuses the incoherent case, which is all it claims) but it is the measure of how far the format is from saying what a clip is.

- ✅ **Does the working copy still lead, now that a write costs the span?** *(opened 2026-08-17, when the server's S18 measured the premise this rests on out of existence)*. O8 says the working buffer leads while a session is open, and a take's pool buffer is replaced whole **once, on confirmation**. That was not a preference: it was derived from a measurement — a write cost the buffer (33.8 ms on a five-minute take), so an editor that wrote through per gesture was unusable, and keeping a client-side copy was the honest workaround. **The measurement changed**: an in-place span write is 0.0001 ms and flat in the material (S18), and with the samples in the shared segment a local peer's write costs no message either (S19). So the question is open again and it is this crate's, because the answer is what a session *means*: is the material the server holds the one copy, edited in place and confirmed by nothing, or does a working copy still lead and confirmation still exist as a step?

  **What argues each way, so the answer is not read off the benchmark.** *One copy* deletes a whole reconciliation — no `editing` state to reopen into, no two pictures of one take to keep in step, no confirmation gesture — and it is what makes the standalone host's three processes share material rather than ship it. *The working copy* is what makes the posture **non-destructive**: an unconfirmed edit is one nothing else can see, an undo has somewhere to read the previous samples from, and a session that dies mid-edit reopens with the take intact. Those are the four-layer table's own rules, and no write cost was ever their reason.

  **The likely shape, stated so it can be argued with rather than assumed:** the distinction survives but stops being about *cost* — a working copy where the edit must be reversible or invisible, a direct write where the host owns the material and the edit is confirmed by the act. Whichever, `editing` in the session format and the confirmation step in the editor are the two things that move, and both are here.

  **Taken 2026-08-17, and the answer is that the working copy leads and there was never a second one to make.** The rule survives; what dies is the reading of it that says an edited take exists twice.

  **The server buffer *is* the working copy.** Loading material into a pool buffer copies it — `/buffer_allocRead` reads a file into a buffer and the file is not touched again — so an edit that writes that buffer has already not written the source. What the four-layer rule protects is *the user's own file*, and it is protected by the copy that loading already made, not by a second copy on top of it. So there is **no confirmation step per edit**: what confirms a stroke is the acknowledgement (O3) and the log entry, which is what the editor already does. A `Lifetime::Session` buffer is edited where it lies.

  **Undo does not need one either, and the crate already says so.** `inverse_of` returns the *empty* write for `WriteSamples` — "the samples are not in the document ... which is why a destructive caller reads its own span before writing" — and the host does exactly that (`read_inverse`: `"sample"` carries the value it replaced, `"draw"` carries the run). So the previous samples live in the **log**, span by span, which is cheaper than a second take and is already built. The cost argument that once justified a working copy was never the one holding undo up.

  **Where a temporary copy is still mandatory, and it is a property of how the material is held rather than of cost:** material reached **by reference to the user's own file** — mapped rather than loaded, which is the path S19/H6 open. There an edit would write the user's file, which the four-layer rule forbids outright, so the edit must materialize a `Lifetime::Temporary` copy first and `confirm`/`promote` are what settle it. That is the whole remaining job of `OpenEdit`, and it is why the vocabulary stays in the format: a session that dies mid-edit over mapped material has to reopen knowing what was undecided.

  **So the two things the roadmap said would move, move like this:** `editing` in the session format **stays**, narrowed to the mapped case and unused until it exists; the per-edit confirmation step **is not built**, because the acknowledgement is it. And the one-tree refactor inherits a simpler world than it would have: one copy of the material, one place an edit lands, and a history that already holds what an undo needs.

- ⬜ **What is the second document: the application, and not the arrangement?**
  *(opened 2026-08-21 by the user, and it is **open and undefined** — recorded
  here to be thought through, not to be answered by the next milestone that
  trips over it)*. The crate's `Document` was thought for **one** thing: the
  arrangement. It holds the arrangement's data structures and the GUI is
  **implicit** in them — a view exists because the objects that represent the
  data can show themselves, which is exactly why the editor needs no layout
  saved anywhere. That property is real and is not what is in question. What it
  is not is an **application scope**, and the absence of one is what this
  question is about.

  **`Document` is the arrangement's, *for the moment*** — that scoping is where
  this decision finds things, not a boundary it has agreed to keep. Whether a
  document is one kind of thing with the application as a second, or one shape
  that both levels are written in, is part of what is open here; so nothing
  should be built on "the crate is the arrangement's" as if it were settled,
  and this line is here so a reader of the sections above does not take it for
  one.

  **Application scope defines a second level, and a second kind of document.**
  An application is a *combination of GuiDefs*, and what saves its form and its
  organization is a document of its own — not the arrangement's. Inside an
  application one then opens a document, and an arrangement is one kind of
  document among the kinds that application deals in. The analogy it was argued
  with is direct: building a text editor, the application scope is the
  combination of GuiDefs that *is* the editor, specified by a document saying
  how it is put together; inside it, what one opens is a text document. Audio
  defs sit on the same axis — they are the application's **processing
  capabilities**, not its contents.

  **So what this project is building is a framework for dynamic applications**
  made of predefined elements (defs) that combine the host and the server — and
  such an application could exist **with no interpreter at all**, which is to
  say with no client. That is the frame the second document has to be designed
  in, and it is why the question cannot be settled from inside a milestone: it
  decides what the crate is for as much as what it stores.

  **What is written down and is not being decided:** whether the second document
  is this crate's at all (it is a document, and a format with two writers in two
  languages drifts — the argument that put the first one here); what it holds
  (the GuiDefs and their composition, the defs the application needs, what a
  "kind of document" is to it); how an arrangement document is *opened inside*
  one; and what any of it means for the standalone host, which is the case with
  no interpreter and therefore the case that decides.

  **The `Session`/`Document` naming is secondary to this and waits on it.**
  There are two `session`s in the tree — this crate's saved file
  (`session::Session`, the document plus its source table) and the client's
  isolated environment (`clausters.Session`: a server, a GUI host, a clock) —
  and the instruction is that the one which is a document be called a document.
  That is right, but *which* thing ends up called what depends on how many
  documents there turn out to be, so it is not worth paying twice. Note that
  `Lifetime::Session` uses *session* in a third sense, the running one, which is
  the client's rather than the file's; whichever names move, that one is decided
  in the same pass so the term is not left half renamed. And a rename here is a
  pass of its own, never a search and replace: it changes the subject of whole
  paragraphs of doc comment in the crate, in two clients and in three books.

  **The GUI plan's "Persistence saves the document, not what the user did to
  it" is the same question arriving from the other end**, and is reformulated
  here rather than answered there *(moved 2026-08-21; the entry stays in
  `clients/gui/PLAN.md` with the record of what was seen)*. What it found: a
  named GuiDef persists as the bytes the script sent, so a control a user left
  somewhere is saved nowhere. What it settled, and what does not need this
  decision: **the value does not go back into the def** — a GuiDef defines an
  interface, holds no data, and a widget is not what saves configuration in any
  application. What it could not settle is where the value *does* go, and this
  entry is why: a standalone bundle is a combination of GuiDefs with no
  application document behind it, so there is no owner to write it to. The
  arrangement's own path is unaffected either way — a gesture becomes a change
  on the arrangement and the document saves that — which is the measure of how
  narrowly this misses being urgent.

  **What the application-scope track built, and what it leaves open**
  *(2026-09-14, closing that track)*. An application layer exists now:
  `crates/clausters-apps` holds the applications — the window each one composes
  and what each gesture in it is answered with — and its editing context holds
  the one undo order over every editor opened in it and the structures the crate
  does not apply. It sits **beside** this crate's document rather than inside
  it, and it saves nothing: an application holds no document of its own, and
  what a window set is made of is still composed at run time by a client or a
  host. So the track answered what an application *does* and left this entry's
  question as it was — whether an application has a document, and whose it is —
  open, on purpose.

- ⬜ **What does a version mean across a load?** *(opened 2026-09-06 on the
  application-scope track, when a load lowered the staleness floor to
  `FIRST_VERSION`; that path went with `FormEditor`, and the question did not)*.
  A floor only rises, which is what makes staleness a monotone test. A load that
  replaces the composition under an open window leaves edits in flight from the
  piece that was replaced, and `max(old, new)` is not obviously right either,
  since the two versions count different histories. Today a context counts per
  context (`clausters_apps::editing::Editing` starts at `FIRST_VERSION`, and a
  session opened again is a new context), which answers it for a new window and
  not for a context whose data is replaced under it. The decision is whether a
  context's counter is per composition or per session.

## The milestones

- ✅ **O1 - The document: the tree, and a leaf is opaque.** The crate's types and their serde form: elements with their placements (`onset`, `duration`), aggregates with their two kinds (**concrete** - members relate in time; **logical** - they relate by processing), and a leaf as `(id, kind, opaque config)`. A **source reference** carries its **lifetime** (external / session / temporary) from the start rather than gaining it later, since it is what a save has to read. No client objects, no widget, no OSC, no I/O. **Two properties the shape has to admit, because they are the arrangement's and not the document's to invent.** A generator's *code* is the opaque leaf; **its output is ordinary tree** - a generator may produce any element, generators included - so nothing about being generated makes a subtree a second kind of thing. And a **clang may reference a generator** to fire it live, which means the document expresses structure resolved at run time and not only at render time: a reference that no flattening pass will ever expand. **The tree stays general, and the views carry their own restrictions** - a multitrack lane is a *projection* that may decline to show what its shape does not admit, exactly as an unknown widget is laid out and not painted; nothing here grows a lane, a vertical position or a type-per-container so that a view is easier to write. **[Withdrawn 2026-09-06 - see "The turn: the arrangement stops being a projection". A projection is not free of structure: the lane, the order within it and the placement's identity are durable and undoable, and with nowhere to live they fell into the widget tree, which is where every multitrack defect of the `application-scope` branch comes from. O1's types are not deleted - they become what a region may contain - but the arrangement stops being derived from a general tree.]** The arrangement's own vocabulary (the Aggregate and the other primitives, the temporal traits) is unchanged by this milestone and is refined by iteration in `clausters.form`, which is why the shape has to stay versatile rather than final: the document need not know the model, but it must not be what blocks its refinement. **Acceptance:** a tree round-trips through serde unchanged; an unknown body kind and an unread config blob both survive a load/save cycle **losslessly**, and writing is **deterministic** (the two are the properties that matter, and byte-identity is not one of them: `serde_json` sorts an object's keys, key order in JSON carries no information, and buying its preservation would mean turning `preserve_order` on for every crate in the workspace since features are additive); the Python `clausters.form` tree converts in and out with no loss.
- ✅ **O2 - The intent vocabulary, and the one applier.** The intent enum - absolute only - and `apply(document, intent) -> Outcome`, where the outcome carries the **effective** value and therefore *is* the acknowledgement's content. Every transformation the owner performs (snap, clamp, a refusal on read-only material) happens here and is reported here, so no caller can apply an intent by hand. **The vocabulary has no relative form and that is the whole of the rule here**; the one payload that violates it is the host's `"transpose" <xml:id> <steps>` on an engraved page (`clients/gui/src/host/elements/score.rs`), and converting *that* is host work rather than the crate's, so it rides with H1, which is already the milestone that passes over every payload the host emits. The absolute form is available to it: the host re-derives staff position from the engraving in order to draw ledger lines, so it can name the position a note reaches instead of the steps it moved. **Acceptance:** applying the same intent twice leaves the same document (idempotence); a refusal reports the unchanged value rather than an error; every outcome names an effective value, and a test enumerates the vocabulary so a new intent cannot be added without one.
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

  **The GUI host did not take the vocabulary, and that is the honest report.** The milestone says "the intent vocabulary **and nothing else**", and today the host constructs no `Intent`: it emits `/gui_event` payloads in the owner's terms and draws what comes back, which is the whole edit-back design. Adding the dependency now would be a declaration with nothing calling it - the same call K5 made about `Element::event()`. The forcing case is a **standalone host that edits**, which is the GUI track's H3, and the dependency lands with it. *(It landed: H3 closed 2026-08-16, and `clients/gui/src/host/document.rs` builds `Place` and `WriteSamples` from `/gui_event` payloads — the prediction held, including that the host would take the vocabulary and nothing else.)*

  **Docs.** `docs/architecture.md` gained the four layers and the crate's place under "The arrangement layer" (with O5); `docs/decisions.md` gained the two entries the milestone names - the single document with immutable sources, including why MVC stops working once a view edits, and the acknowledgement as a stamped state push rather than a reply code; `docs/bindings.md` gained the document section with the no-handles reasoning; and the Python book's composition chapter gained "The document: what the composition *is*, and who edits it" - the conversion, `document_apply`, and sessions.

  **Not done, and named rather than implied:** `clausters.form` is still this client's own object model with a conversion to the document, not accessors *over* the crate's tree. The conversion is lossless and id-stable and the crate is normative through it, which is what the decisions asked for ("the clients round-trip; they do not hold handles"); turning the Python objects into views over a shared tree is a further refactor with no consumer waiting on it. *(It has one now, and it is cost rather than ergonomics: **O12** measures what "a serialization per edit" costs on a real composition and is where that refactor is decided.)* The **log** (O5) is also unbound - it is stateful, and whether it crosses by handle or by value is a real trade rather than a detail - so that is **O11** below rather than a note here, because the GUI track's H2 depends on it.

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


## The turn: the arrangement stops being a projection

*(Taken 2026-09-06, by the user, after a day of defects in the multitrack view
and a reading of how the field builds one. It changes O1's central premise, so
it is written here rather than inside the milestone that would carry it.)*

O1 settled that **the tree stays general and a view carries its own
restrictions** - *"nothing here grows a lane, a vertical position or a
type-per-container so that a view is easier to write"* - and that a multitrack
editor is a **projection**. That was a defensible reading of what a document is,
and it is now withdrawn, for a reason the branch discovered rather than argued:

**A projection is not free of structure.** The multitrack's projection carries
real, durable, undoable state - which lane a thing is on, its order within the
lane, its placement, its identity - and none of it was in the document, in the
crate or anywhere else. So it fell, by elimination, into **the widget tree**,
which is the one place it may not live, because a widget tree is drawn and
drawing frees. Every defect the `application-scope` branch turned up is a
consequence: a clip that changes lane is a re-parent of a UI object; a clip that
appears is a change of shape on a wire; and a lane's zoom dies because the only
way to say *a clip arrived* was to rebuild the lane.

The evidence that this is structural and not a bug is the **piano roll**, which
never has any of these defects. Its contents are **data inside one widget**
(`Vec<Note>`, five numbers, contiguous, selected by index) rather than widgets
inside a tree, so the whole class - identity, staleness, lost screen state -
cannot occur in it. The host already knows the two are one thing where it counts
(`placement.rs`: *"one geometry for every box that lives on a time axis"*; a lane
and a semitone row are one `Bands`), and deliberately keeps the storage apart
because a `Widget` is 13.5x a `Note`. What was missing is the lane's own
equivalent of `Vec<Note>`.

**One correction, so this is not over-read** *(2026-09-06, from a pass over open
implementations)*. **A widget per clip is not the defect.** LMMS has exactly
that - a `Clip` model and a `ClipView` widget per clip, the model emitting
`dataChanged` and the views following - and it is ordinary MVC that works. What
makes ours fail is narrower and worth stating precisely: **the widget tree is the
only copy of the structure, and it lives on the far side of a wire.** LMMS can
afford a widget per clip because the model is authoritative and in-process, so
the widget is a view *of* something. So `O23` does not have to stop drawing a
clip as a widget; it has to stop the widget tree being the only place the
structure exists.

**So the arrangement becomes a first-class structure in this crate**, modelled on
what the field has converged on over forty years, and the three classic
applications - the **audio editor**, the **multitrack editor** and the **score
editor** - become what the project is *for*, each an application over this one
document rather than a view over a client's private model.

**`clausters.form` is not that model and stops being treated as one.** It is
retained as a small set of client-side data structures, frozen, with no GUI and
no milestone; `FormEditor` is removed outright. It was the arrangement's
iteration surface while there was nothing else, and it did that job; what it is
not is the shape a professional multitrack is built on, and continuing to project
one out of it is what this turn ends. The documentation stops giving it
prominence.

### What the model is

Named as the field names it, because the vocabulary is settled prior art and
inventing our own would cost every reader the translation.

- **Source** - the material, immutable, with the `Lifetime` and the generation it
  already carries. Audio samples, an event sequence, or an **opaque leaf** (a
  generator: a def, a pattern, a routine). Unchanged from O1 except that it is
  now referenced by regions rather than placed directly.
- **Region** - a **window onto a source**: its own identity, a reference to what
  it plays, `position` on the timeline, `length`, and `start` (the source frame
  its own zero reads). Plus what a professional region carries and ours does
  not: **fades** (in, out, and the crossfade with a neighbour), **gain**, and its
  **layer** where regions overlap. Several regions may reference one source; that
  is the whole of non-destructive editing.
- **Lane** - the ordered regions that **are a track's contents**. A track holds
  **several** and plays one, which is what takes, comping and alternate versions
  are, and which this project has never had. Ardour calls this a *playlist* and
  the structure is its; the name is not, because in ordinary use a playlist is a
  list of songs and the word has to be decoded before it means anything here.
  `Lane` says what it is, and a track with three lanes **draws as three rows**
  when expanded, so the model word and the view word are one word about one
  thing. *(Decided 2026-09-06; the reasoning and the cost - `lane` is used 1596
  times in the host in two senses, one of which has to be renamed to `channel` -
  are in `docs/decisions.md`, "A track's contents are a lane, not a playlist".)*
- **Track** - identity, name, colour, kind (audio / MIDI / bus / folder /
  master), its lanes, its **automation** curves, its routing (inputs, outputs,
  sends) and its authored state (gain, pan, mute, solo, arm). A **folder** track
  contains tracks, which is the only recursion at the top level.
- **Automation** - a curve per addressable target (a track's parameter, a
  region's, a plugin's), holding break points. Authored and undoable, never
  derived. **Three attachment levels is right**: REAPER's envelopes name a track,
  an item or a take as their parent, which confirms this rather than leaving it a
  guess (see the design reference).
- **The session** - the tracks in order, the **tempo map** and **meter map**,
  **markers**, **ranges**, the loop, and the routing graph.

**Where the general tree survives, and it is not deleted.** O1's recursion moves
inside a region: a region may reference a **composite** rather than a flat
source - a nested timeline, which is OTIO's `Stack` and which Ardour lacks. That
keeps every property O1 argued for (a generator may produce any element,
generators included; a clang may reference a generator; an unknown body survives
a round trip) and stops the top level from paying for them. The top level is a
DAW session, because that is what a DAW session is good at.

### The decisions this turn takes, and what each replaces

- **A region has its own identity, distinct from the source's.** This **answers
  the open decision above** ("May one element be placed twice, and what does an
  intent name if it is?"), and answers it with the third option, *name the
  placement* - which that decision had already argued was the only survivor once
  read against what a multitrack is. A region is the placement, given a name.
  The instance/function typing it also asked for lands on the **source**: an
  instance is a thing two regions may fork, a function is an algorithm two
  regions evaluate, possibly with different arguments, and a region carries the
  arguments of *its* evaluation.
- **A track's contents are a list it owns, not children of a widget.** A region
  changing track is *remove from lane A, insert into lane B* - two list edits in
  one transaction, which the log already expresses (O17). **No widget is created
  or destroyed**, so a track's zoom, scroll and selection survive by construction
  rather than by a redefine narrow enough to spare them.
- **The document holds the session; the host binds it.** This changes nothing in
  the four-layer table and is the `standalone` mode the crate was designed for
  from the start. The host holds no *durable* data: it holds the session the way
  it already holds a roll's `Vec<Note>` - editing it, drawing it, reporting what
  the hand did - and the durable copy is the document's. Screen state stays the
  host's and stays out of the document.
- **What the field does that we will not copy.** OTIO makes empty space an object
  (`Gap`) so a track is a sequence with derived positions; we keep **absolute
  positions**, as Ardour does. That is the NLE family against the DAW family, and
  we are building a DAW. Recorded so the choice is known to be one.

### Design reference: what the field does, and where we differ

*Read 2026-09-06 on the application-scope track, to check its conclusions against
prior art rather than to derive them. Context the milestones below are read
against, not work: what turned into work is in them, named.*

- **Item and Composition.** OpenTimelineIO splits a leaf (clip, gap, transition)
  from a container (a track orders its children in time, a stack in parallel).
  Our aggregate's two kinds are concrete and logical — a different axis, chosen
  knowingly.
- **Empty space as an object.** OTIO's `Gap` makes a track a sequence (the NLE
  family, where ripple editing falls out); we keep absolute positions, as Ardour
  does (the DAW family), because this is a DAW.
- **Source → Region → Playlist → Track.** Ardour's four levels are the model the
  turn takes: an immutable source, a region that windows it and has an identity
  of its own, an ordered list of regions that *is* a track's contents (named
  `Lane` here — `docs/decisions.md`), and a track that plays one of several.
  Moving a clip between tracks is then a list operation, not a re-parent.
- **A clip's three numbers.** OTIO's `source_range`, REAPER's position, length
  and source offset, and Ardour's region are our placement, length and start —
  three independent systems agreeing.
- **The model is authoritative and the view is reconciled.** kdenlive binds its
  QML timeline to a C++ model through `DelegateModel`; Ardour builds its canvas
  views from the playlist. That is the host's reconcile under its industry name,
  and the split of state — positions and structure in the model, selection and
  drag previews in the view — is the four-layer table again.
- **Identity by id, order by index.** Tracktion finds a clip by id and indexes it
  by position; the reconcile matches the same way.
- **REAPER: the take on the item, and a change of mind.** Its levels are
  Project → Track → MediaItem → Take → PCM_source, with automation at the track,
  the item and the take. Takes inside the item answer *an alternative* and do not
  scale to *assembling a composite*: REAPER 7 added Fixed Item Lanes on top and
  both now coexist, which is why this crate takes the lane shape. Its `.rpp`
  writes a source inside each item; a source table plus references, as here, is
  what lets six regions share one source without repeating it.
- **The view in the open programs.** LMMS keeps a widget per clip and it works,
  because its model is authoritative and in-process — the defect here was the
  widget tree being the only copy of the structure, across a wire. Zrythm's
  layering and object registries match ours, and its strong timebase types went
  into `O21`. Live keeps view objects parallel to model objects (`Song.View`,
  `Track.View`), which `O23`'s view took, and shows two pictures over one model
  (Session and Arrangement), which says a presentation is per view. Ardour's
  canvas is a retained scenegraph with three coordinate spaces and no dirty
  regions, built without scaling because *"single pixels have semantic content"*
  — this project's never-resolve-finer-than-the-screen rule, reached
  independently.
- **What nobody else does.** Every one of them keeps model and view in one
  process; none sends a view tree over a wire per redraw. The nearest analogues
  are reconcilers (the DOM, Qt), which is the second road to the same answer.

**Sources.**
[OpenTimelineIO data model](https://deepwiki.com/AcademySoftwareFoundation/OpenTimelineIO/2.2-timeline-data-model) *
[kdenlive timeline UI](https://deepwiki.com/KDE/kdenlive/3.2-timeline-ui) *
[Ardour, working with regions](https://manual.ardour.org/working-with-regions/) *
[Tracktion `ClipTrack`](https://github.com/Tracktion/tracktion_engine/blob/master/modules/tracktion_engine/model/tracks/tracktion_ClipTrack.h) *
[ReaScript API](https://www.reaper.fm/sdk/reascript/reascripthelp.html) *
[Fixed Item Lanes](https://forums.cockos.com/showthread.php?t=283665) *
[The Ardour Canvas](https://ardour.org/canvas.html) *
[LMMS architecture](https://github.com/LMMS/lmms/wiki/LMMS-Architecture) *
[Zrythm architecture](https://deepwiki.com/zrythm/zrythm) *
[The Live Object Model](https://docs.cycling74.com/legacy/max8/vignettes/live_object_model) *
[Bitwig clip launcher](https://www.bitwig.com/userguide/latest/the_clip_launcher/)

### The milestones

- ✅ **O21 - The session: the types and their format.** *(Closed 2026-09-06.)*
  Source, Region,
  Lane, Track, Automation, Session, and the tempo/meter maps, markers and
  ranges - serde, round-trip, unknown-field preservation, and the same
  determinism O1 accepted. The composite region carries O1's tree unchanged. No
  intents yet, no wire, no host. **Acceptance:** a session with several tracks,
  alternate lanes, overlapping layered regions, crossfades and automation
  round-trips losslessly; a document written by a newer writer survives a
  load/save. *(An earlier acceptance line also asked that "an O1 document
  converts into a session"; withdrawn 2026-09-06 - see (c) below, which decides
  whether such a conversion exists at all.)*

  **What landed, 2026-09-06.** `clausters_document::timebase` mints one type per
  axis - `Beat`, `TimelineFrame`, `ContentFrame`, `ContentBeat` - with the
  arithmetic that stays on an axis and **no conversion between axes**, since a
  beat becomes a frame only through a tempo map and a rate and both belong to
  whoever holds them. `clausters_document::arrangement` holds `Region`, `Lane`,
  `Track`, `Automation` and the timeline they sit on (`Tempo`, `Meter`,
  `Marker`, `Span`, `Arrangement`), and `Session` carries an arrangement beside
  the general tree, written only when there is one so every session saved before
  this reads back unchanged. Unknown fields survive on every struct, which serde
  does not do by default. The acceptance is one test - three tracks, a vocal
  comped from three takes playing the second, two guitar regions overlapping
  with a crossfade and a layer order, a composite region placing the general
  tree, over a ramping tempo map, a meter change, markers, a loop and a punch,
  with an automation curve whose point shapes this crate does not read - and it
  reopens equal. The tempo map has an owner at last: the **piece**, which is the
  fourth candidate the roadmap's ownership entry named and the one it expected.

  **And the client half, the same day.** `clausters.arrangement` and
  `arrangement.ts` write and read a piece and a session; `clausters.document` is
  the door the Python client did not have, since the surface lived in the
  private `_native` while the web client had `document.ts` all along - one
  client with a door and one with a back way in, which is exactly the asymmetry
  the non-divergence rule exists to catch. The crossing is one piece and three
  readers: a vector the Python client builds, the crate parses and the web
  client reads and rewrites, plus the same piece as a session with its table.
  `form`'s door is deleted (e), and what was worth lifting from it was the
  session half and never the conversion (c).

  **The consequence found by measuring before deleting, and worth keeping:**
  `form` was the **only writer of the general tree in any client**, so `Body`
  was about to lose its crossing along with its writer. It did not, because a
  composite region already carries a `Node`: the arrangement's vector exercises
  the tree inside one, and `form_parity.rs` was replaced rather than merely
  removed. The general rule behind it is that a format keeps its crossing as
  long as *something* a client can write reaches it, and the composite region is
  now that something.

  **The contradiction this milestone actually resolves, named because nothing
  else in this file names it** *(raised 2026-09-06 by the user)*: **the only
  document that ever existed is `form`'s.** `Body`'s variants are `Clang`,
  `Sequence`, `Vector`, `Track`, `Generator` - `clausters.form`'s five
  primitives, given a serde form - and the only door into the crate from either
  client is `form/document.py` and `form/document.ts` (`to_document`,
  `from_document`, `to_session`, `from_session`). So the crate is complete, and
  the model it is complete *for* is the one just relegated, reached only through
  a frozen module. Three things sit on that door today and none of them is
  `form`'s: the standalone host opening a session, `editors/session.py`, and the
  whole save/reopen loop.

  **What that means concretely, and it is bigger than a rename.** An earlier
  draft of this entry asked *"what is a `Sequence` of `Clang`s as a session?"* -
  a lane of regions, one region per clang; one region over the whole sequence;
  nothing at all, since a generator has no source to window. **That question is
  withdrawn** *(2026-09-06, by the user)*, and it was the wrong one to ask:
  answering it is precisely how the new model would inherit the old one's shape.
  `form` is not a source of design here. It is not part of any structure of the
  GUI - `FormEditor` is gone, and nothing the host draws is shaped by it - and it
  is not the thing `O21` converts from. **The session types replace `Body`; they
  are not added beside it.**

  What `form` leaves behind is one asset and three tasks:

  **(c) `form`'s document is discarded; what is reused is its round trip, and
  reused means for the other documents.** *(Done 2026-09-06: the session half
  was lifted - `Source`, `FrozenSource` and `Session` in both clients - and the
  element-to-node conversion was discarded with the module. What made the split
  obvious once the file was read rather than counted is that the lifted half
  never mentioned form's primitives.)* `form/document.py` (1268 lines) and
  its TypeScript twin are a working round trip through the crate's format: id
  stamping, opaque payloads,
  generators by reference, unknown-field survival, the version reservation. That
  machinery is general and the *vocabulary* it carries is not. The file goes
  either way - see (e) - so what is decided here is only whether the round trip
  is **lifted before it goes and reused as the shape for the several documents
  this project now has**, or written again from nothing: the multitrack
  editor's session, the audio editor's, the analysis layers of the audio editor
  (Sonic Visualiser's panes and layers are a document too, see `O24`), the score
  editor's. Nothing about that reuse keeps `form`'s five primitives; what is
  reused is the round trip.

  **(d) The door moves out of `form` entirely** *(decided and done 2026-09-06;
  the Python client gained `clausters.document`, which it had never had)*. Not a second door beside the old one, not a re-export, not a
  compatibility shim: `form` ends this milestone with no path into the crate.
  Today the crate's whole client surface - `edit(x)`, the document, undo,
  selection, the clipboard, save and reopen - is reachable only through
  `form/document.py` / `.ts`. Three callers sit on that
  door and none of them is `form`'s: the standalone host opening a session,
  `editors/session.py`, and the save/reopen loop. The session types have nobody
  to talk to until this module exists on its own, in both clients, so it is part
  of `O21` and not a follow-up. A Python change, a TypeScript change and a book
  page in each - the pass over the packages, in the same milestone.

  **(e) `form` keeps no door, and leaves no residue** *(decided and done
  2026-09-06: 6157 lines out, 619 in)*. It does not write a session, not even one way out. `form/document.py`
  and `form/document.ts` are deleted, `ID_ATTR` and the id stamping go with them,
  and `form` ends with no relation to the crate at all - which is what "frozen,
  relegated, secondary" already implied. Whatever of the round trip is worth
  keeping is lifted first, under (c), and lands in the new module as its own
  code; nothing is left behind in `form` as a forwarding stub or a deprecated
  alias. `clients/python/docs/src/form.md` and its web twin say the module has
  no document, rather than saying where the document went.

  **Strong types per timebase, and they are cheap.** Zrythm's 2026 arrangement
  overhaul added a `Position` primitive plus **strong `ContentTick` /
  `TimelineTick` types** for exactly the confusion we have: a session carries
  **three** time axes - beats (musical), timeline samples (the view's), and the
  frame of a source a region's `start` reads - and today nothing but a comment
  says which is which. It has already cost us once: a threshold computed on the
  wrong axis turned every clip move into a trim. Newtypes make that a compile
  error, and this is the milestone that mints the types.

  **Two things it has to settle rather than assume.**

  **(a) Is a region one object or two?** *(Settled 2026-09-06.)* REAPER splits
  the slot in time (`MediaItem`: position, length, fades) from what fills it
  (`MediaItem_Take`: the source reference, its offset, its playrate, its own
  envelopes), and that is the same distinction the open decision above closed as
  *name the placement*. Splitting gives comping by construction - swap what fills
  the slot, keep the slot - and costs a level in the format and in every intent.
  **The answer is one object**, and the argument that settles it is not the cost:
  **we already took Ardour's lane, and takes are how REAPER does what a lane
  does.** A track holding several lanes *is* the comping mechanism, so the split
  would give us a second one at a different level, for the same job. The field
  itself says so - REAPER 7 (2023) added **fixed item lanes**, described by its
  own users as the alternative to recording into takes, and shipped an action
  named *convert takes to lanes*. Adopting the split now would be adopting the
  thing its author has since grown a lane model beside.

  A second reason, and it is this crate's own: the decision above spent itself
  establishing that **the placement has one identity**. Splitting reintroduces
  exactly the question it closed - which of the two an intent names - one level
  down, and the *placed twice* defect is what that ambiguity cost the last time.

  **So a region is one object with a typed `content`**, not a level: the timeline
  span is the region's (position, length, fades, layer), and what fills it is a
  field on it that carries the source reference, the window into the source, the
  playrate and the arguments of its own evaluation. That is the same split
  REAPER draws, expressed as **types rather than as a nesting** - which is where
  the timebase newtypes below do their work, since the two halves of a region are
  measured on two different axes and today nothing says so.

  **What this costs, said plainly rather than discovered later.** Swapping what
  fills a region does not keep the fades, because there is no slot to keep them
  in - the fades are the region's, and a swap is an edit of the region. And
  alternatives to one span live in alternate lanes, never inside one region. If a
  case ever appears that lanes genuinely cannot express, it reopens **here**,
  with that case named.

  **`Region` is the model's word and `clip` is the picture's, and both stay.**
  The project's rule already says a clip, a lane, a roll and a waveform are
  *views*; the wire already gets it right, since a `field` with a placement is a
  clip and nothing on it names an "audio clip". Zrythm made this same turn and
  **merged** the two, renaming `Region` to `Clip`; we keep them apart on purpose,
  because the thing the host draws and the thing an intent names are not the same
  thing and the multitrack's defects came from treating them as one.

  **(b) `lane` means two things in the host today** - a track's row, and a
  *channel* row inside a multichannel clip body - and the second has to be
  renamed here, because the first is what this milestone makes the model's word.
  **Done 2026-09-06, before any type was written**, so the model's `Lane` lands
  in a host where `lane` already means one thing. What the rename found is that
  the second sense was **two** senses, not one, and they took different words:
  `channel_rect` / `channel_at` / `stft_channels` / `channel_divider` where the
  thing really is a channel, and **`rows`** where the count is generic - an
  element states how many rows it stacks (`Element::rows`, `ValueAxis::rows`,
  `YAxis::rows`/`row_h`), which is one per channel for a signal view and one for
  an overlaid one, and would be a semitone row for a roll. `Element::channels`
  already existed and meant *how many channels the data holds*; the two are not
  the same number the moment a view overlays, which is exactly the confusion the
  shared name was hiding. The one surface that moved is the theme role
  `lane_divider` -> `channel_divider` (it only ever drew between stacked
  channels), with its book table. `lane` now appears in the host only where a
  track's row is meant.
- ✅ **O22 - The intents the session admits.** *(Closed 2026-09-07.)* The vocabulary extended to what a
  DAW does: place, move, trim, split, join, fade, crossfade, set layer, move
  between lanes **and between tracks**, switch a track's active lane,
  add/remove/reorder tracks, edit an automation lane, set a marker or a range.
  Absolute, idempotent, applied only here, each reporting its effective value -
  O2's rules unchanged, its vocabulary widened. **Acceptance:** every intent
  enumerated by a test; a region moved between tracks is one transaction that
  undoes in one step; a trim reports the effective placement after snapping.

  **(a) The vocabulary, and where it lives.** *(Landed 2026-09-06.)* Fourteen
  verbs in `arrangement::edit::ArrangementIntent`, beside the model rather than
  inside `intent.rs`: the piece is **its own editable domain**, registered as
  `ARRANGEMENT` alongside `tree`, `points`, `samples` and `events`, with
  `edit::Piece` as its [`Editable`] exactly as `log::Tree` is the document's.
  That is what the domain table was built for, and it means the piece's edits
  and the tree's share one undo pile without either knowing the other's words.

  The list, and the three groupings it fell into: what a **track** is
  (`SetTracks` whole - add, remove and reorder are one verb because all three
  state the same thing - and `SetActiveLane`, which is comping's one verb);
  what a **region** is (`SetLane` whole, `PlaceRegion`, `TrimRegion`,
  `SplitRegion`, `JoinRegions`, `FadeRegion`); and what the **piece** holds
  (`SetAutomation`, `SetMarker`, `RemoveMarker`, `SetRange`, `SetTempoMap`,
  `SetMeterMap`).

  **A move between tracks is one intent, not a transaction.** The acceptance
  asked for one transaction that undoes in one step and the vocabulary does
  better: because an intent is absolute, and *where a region is* is three
  coordinates (track, lane, beat), `PlaceRegion` states all three at once. So
  there is one entry, one undo, and - the part a transaction would not have
  given - **no state in between where the region is on no lane at all**, which
  every reader that redrew mid-gesture would otherwise have seen.

  **The two maps got verbs the list did not ask for.** The tempo and meter maps
  are the piece's (O21's decision) and no verb reached them, which is exactly
  the defect `Intent::Configure` was widened to fix on the tree side: an edit
  nothing can describe is an edit nothing can invert.

  **(b) The two verbs this crate cannot compute.** *(Found 2026-09-06, while
  writing them.)* Splitting and joining a region are the only edits that change
  how many regions there are, and they are also the only two the crate cannot
  work out on its own: **the cut is on the musical axis and a window into a
  source is on the content's**, and `timebase` converts between the two never -
  that is its whole premise, and it is not suspended because it would be
  convenient here. So the caller, which holds the tempo map and its own frames
  per beat, states the halves' content (`left_content`/`right_content`, and
  `content` on a join) and the crate does the rest. `None` leaves a half reading
  what the region read, which is right for a composite and for a caller that
  does not care.

  Both also **invert as `SetLane`** - the lane's previous contents, whole.
  Nothing smaller describes putting back a region that was made out of two, and
  computing it back would be the same refused conversion in the other direction.

  **(c) The piece carries its own version.** *(Decided 2026-09-06.)* Staleness
  needs a counter and `Document::version` is the tree's. A shared one would make
  every edit to either half look like a change to both, and an editor of the
  piece is not editing the tree - so `Arrangement::version` is a second counter,
  and it is the one that stays when the tree comes off. It stays out of the file
  while it is the first version, so an unedited piece still writes `{}`.

  **What this also fixed, which was prose.** `log.rs` called the tree's
  vocabulary *"the arrangement's vocabulary"* throughout - true when it was
  written and wrong the day `Arrangement` became a type. Renamed to *the tree's*
  wherever the tree is what is meant.

  **(d) The client half cost one constant.** *(Landed 2026-09-07.)* Because the
  piece is a **domain**, both clients already had the door: `domain_edit` /
  `domainEdit` take a vocabulary name, a state and a payload, and hand back the
  new state with the edit that puts it back. So the whole port is the name -
  `ARRANGEMENT` in `clausters.document` and in `document.ts` - plus
  `Arrangement.version` on both dataclasses. **No new FFI symbol, no new wasm
  export, and no typed intent builders in either language**, which is the same
  answer the tree gives: an intent is a value, and a client that has to
  construct one through a class is a client whose spelling can drift.

  That is worth stating as the design's own argument rather than as luck. The
  reason `domain::edit` serves the piece and refuses the tree is not a
  convenience: **a piece's whole state is one JSON value the caller holds**,
  version included, so applying against what that state says and snapping to
  nothing is exactly what a client that just read the piece wants. The tree
  cannot be served that way because what it edits is a *handle* that lives
  across the seam. The old wording said the tree needed a version and a grid;
  the piece needs both too, and has them, so that wording was naming the wrong
  reason. Corrected in the docstring and in `domain.rs`.

  **The crossing.** `document-vectors.json` gained three arrangement rows -
  the move between tracks, a split, and a refusal - written by the Python
  client through the C ABI and replayed by the web client through wasm. One
  piece, two languages, one crate: the parity a vocabulary needs and the only
  check that would notice a binding doing arithmetic the crate is not.

  **What it found on the way.** The web client had no `FIRST_VERSION` and no
  `SESSION_FORMAT`; the Python client had both. Invisible until the piece needed
  a default version to write against, and fixed in the same commit.

  **What O22 leaves for O23.** The vocabulary exists and both clients reach it;
  **the host still does not speak it** - it draws the general tree, and
  `reparent_clip` still moves a `Widget` between two `children` vectors. That is
  O23's whole subject, and it now has something to reconcile against.
- ✅ **O23 - The host binds the session and reconciles.** *(Closed 2026-09-07, but for the web client's half, which is `W30`.)* The host holds the
  session, derives its presentation from it, and answers a change by
  reconciling - matching regions by identity within a lane - rather than by
  freeing and rebuilding. `/gui_def` comes to mean *make it look like this*
  rather than *free this and build that*. **This is what
  the application scope's `AP5` was reaching for** (`clients/gui/PLAN.md`), and it lands here because
  reconciling needs something to reconcile against, which is O21.
  **Prerequisite**: what is the host's and survives a reconcile. It was scoped as
  *a list, written before the work starts*; Live answers it better and by
  construction. In the Live Object Model, `Song.View`, `Track.View` and
  `Application.View` are **objects parallel to the model objects, not children**:
  the model holds functional data, the View holds presentation, and a script can
  read and write both. So the prerequisite becomes a **structure rather than a
  list** - a named view object per model object, whose fields *are* the things
  that survive - and it answers a second question we had not asked, since screen
  state stops being an anonymous blob inside the host and becomes something a
  script can save with a session, restore, and set. That also makes the four-layer
  table's "presentation" row an addressable thing rather than a policy.

  **(a) The prerequisite exists.** *(Landed 2026-09-07.)* `view.rs`: a `View` is
  one window's picture of one piece - where it is looking (`visible`, which is
  the zoom and the horizontal scroll, one fact and not two), `scroll`, `quant`,
  `autofit`, `selection`, `selected`, `focused`, `detail` - plus a `TrackView`
  (`height`, `collapsed`, `lanes_shown`, `color`) and a `LaneView` (`height`)
  looked up **by id**. Every field is one the host already holds or the `AP5` list (`clients/gui/PLAN.md`) already named; nothing was invented to fill a shape.

  Three things are decided by where it sits rather than by what is in it:

  - **Parallel, never a field.** An `Arrangement` serializes byte for byte
    whether or not a view of it exists, which is the property that keeps the
    model clean and is asserted as its own test. Nothing in `view.rs` is ever
    consulted by an edit.
  - **There is more than one.** `Session::views` is a **list**, because a piece
    drawn in two windows has two views that disagree on purpose - the arranger
    snapping to a bar, the editor below it to a sixteenth. A format holding one
    would push the second back to being anonymous, which is the thing this
    structure exists to stop. It is also this project's own reading of Live's
    two pictures over one model, recorded in "Design reference" above.
  - **State goes when the thing goes.** `View::prune` drops every entry naming
    an object the piece no longer holds, including a `selected`/`focused`/
    `detail` that pointed at one. That is `AP3`'s lesson (`clients/gui/PLAN.md`) in this structure's
    terms, and the reason it is a method rather than a habit: keeping too much
    is worse than today's defect, since a height kept for a track that is not
    the same track is a defect that looks like a feature.

  **A written decision had to be refined, and it is named here rather than
  quietly stepped over.** `AP3`'s acceptance says *"nothing about screen state
  reaches a history or a file"*. It still holds where it was aimed - the
  **document** and the **history** - and a view reaches neither: it is not
  edited through an intent and an undo never puts a scroll back. What it no
  longer holds is the *file*, because O23's own framing asks for exactly that:
  screen state a script can save with a session, restore and set. The two are
  compatible only because a session file is not the document, and the view is a
  third thing beside both.

  **The crossing.** The session vector now carries two views, written by the
  Python client, parsed by the crate, read back by the web client - and the pair
  disagrees, so a reader that collapsed them into one would fail rather than
  pass. `arrangement_parity.rs` also asserts that dropping every view leaves the
  piece byte-identical.

  **(b) The host reconciles.** *(Landed 2026-09-07.)* `/gui_def` over a tree the
  host already draws no longer replaces it: `widget::reconcile` walks the held
  tree beside the new one, matches widget to widget, and carries the host's own
  state across. Both entrances do it - a window root and a subtree spliced in
  place - so a def is *make it look like this* whichever door it came through.

  **What decides that two widgets are the same widget.** Identity by id, order
  by index, both - Tracktion's rule for finding a clip, and ours. A widget with
  an id is matched by it **anywhere in the tree**, not among its old siblings,
  which is what makes re-parenting a clip cheap and is why this is a reconcile
  and not an addressing scheme: an id survives a move and a path does not, and
  moving a clip between lanes is the multitrack's most common gesture. A widget
  with no id - a clip's bodies, which the wire deliberately does not address -
  is matched by position among its siblings of the same kind. A match also
  requires the same **kind**, compared as the *wire* spells it: the registry
  already keeps one type string per registered id, read before the def
  overwrites it, and it is the only comparison that separates two elements the
  typed tree spells the same way (`Custom`).

  **What survives is the host's own state and nothing else** - the window on the
  axis (`view_start`/`view_len`, `y_start`/`y_len`), the selection
  (`sel_start`/`sel_len`, `sel_min`/`sel_max`, and the per-widget mark a marquee
  left), the active layer and what is hidden. Eight prop keys and three fields.
  Short on purpose: keeping too much is worse than the defect it replaces.

  **And the def still wins where it says something.** Carrying the host's value
  *over* a value the def stated would take away the one channel a script has for
  moving a view, so the rule is the wire's own - **nothing said is nothing
  written**. A key the def states is the def's; a key it leaves out keeps what
  the host had. That is this milestone's division in one sentence: *the client
  says what it redrew; the host decides what that costs.*

  **(c) The bulk is kept by being asked for.** *(Landed 2026-09-07.)* A def has
  to name every widget in the subtree it redraws, and a clip's samples are the
  largest payload in the system - so a lane restated because one clip moved
  carried every other clip's audio with it, which is the same failure as freeing
  a zoom for it, one order of magnitude up. `"data": "keep"` names the run the
  host is already drawing: the reconcile carries it and the resolved pyramid,
  both behind an `Arc`. It is a **value of the prop that already names the
  samples** rather than a sixth carrier, because a blob is how `data` travels;
  and it is asked for rather than inferred from silence, because silence already
  means something else here - a clip that states no source has no take body at
  all. A keep the host cannot honour is reported, since an empty waveform looks
  exactly like a waveform of silence.

  **(d) The client stops holding a picture.** *(Landed 2026-09-07.)* The
  measurement that gated it is in `docs/decisions.md`, "The picture has one owner": a
  drag's delta is 21 B/frame and flat, the widget the edit named is 91 B and
  flat, the window is 1.9 kB to 652 kB and grows with the piece. It was expected
  to wait on `O24`'s multitrack, since `Application.publish` had no caller and
  rewriting it would be designing a seam against zero implementors. What forced
  it instead was `AP5`'s own item (4) (`clients/gui/PLAN.md`): retiring the redraw difference takes
  `gui_difference` out of the C ABI and the wasm, so `publish` cannot compute one
  whether or not anybody calls it.

  What resulted is **less** machinery rather than a seam designed for nobody:
  `publish(widget, tree, window=…)` sends one `/gui_def`, and `_published`,
  `published()` and `forget_window()` are gone with the picture - and with them
  the window-that-closed special case, since nothing is remembered and so there
  is nothing to forget. The granularity did not become the client's, it became
  the **caller's**: `/gui_def` names any widget, so an editor that knows which
  node its intent touched knows which widget to publish, and one that does not
  can still publish the window and pay for it.

  **And the difference was retired rather than lowered.** It was to move into
  the host; there is nowhere in the host for it. A difference compares two
  *documents* and is correct only when one equals the picture on screen, and
  inside the host that copy is no more reachable than in the client - the host
  writes to its **widgets** without writing to the document it was handed, so a
  redraw restating an offset the document already carried would diff as
  unchanged and leave the widget where the hand left it. The comparison that is
  always true is the document against the **widget tree**, which is (b). Core
  ABI v42.

  **What is left of O23: nothing but the web client**, which has neither the
  `Application` that publishes nor the `GuiHost.redefine` a part is published
  through. Both are `W30` in `clients/web/PLAN.md`, written to run after this
  branch and after `O24` - so the acceptance holds for the crate, the host and
  the Python client, and the clause about every client is what `W30` closes.
- ⬜ **O24 - The three applications.** The **audio editor**, the **multitrack
  editor** and the **score editor**, each an application over this document,
  programmable from the GUI host and driven identically from every client. This
  is what replaces `FormEditor`, and it is where the project's shape stops being
  "a client's model with a view over it".

  **The audio editor is also an analysis tool, and Sonic Visualiser is the shape
  for it** *(wanted 2026-09-06 by the user)*. Its model is two words. A **pane**
  is a scrollable canvas over a time axis; a **layer** is one of a set of things
  shown on that pane, stacked like layers in a graphics application. Every layer
  on a pane shares its horizontal zoom and time alignment, and their **vertical
  scales need not match** - aligned by default only when their units agree - and
  panes stacked in a window align on the same sample frame at their centres.

  **A region carries curves of its own, since 2026-09-09** — the first piece of
  the multitrack editor's own model to land against this milestone, and it is
  one field: `Region.automation`, the same `Automation` a track already
  carries. What tells the two apart is not what they *are* but **where they hang
  and how far they run**: a track's runs the length of the track and is drawn in
  a lane beside it, a region's runs the length of the region and is drawn
  **inside** it. A clip that has curves is a small track acting on itself alone.

  One type in two places rather than two types, and the reason is the rule this
  file already states about naming the structure: what a curve *is* — a target
  in the caller's terms, points on the musical axis, whether it is shown — does
  not change with its scope. A second type would be a second vocabulary, a
  second domain and a second editor for the same picture. The verb did not move
  either: `SetAutomation` addresses a curve **wherever it is**, so widening the
  lookup was the whole of it, and the inverse came for free.

  It is what the GUI wire needs before a box can draw an editable layer, which
  is why it landed first: the wire can carry a curve a client cannot save.

  What that buys, and it is the reason it is wanted here: the layers that
  **display audio** (waveform, spectrogram, spectrum, colour 3D plot) and the
  layers that **annotate it** (time instants, time values, notes, regions, text,
  images) are the same kind of thing on the same axis, and the difference is only
  that the first are read-only because they *are* the audio and the second are
  drawn, edited and erased by hand. An analysis result and a hand annotation are
  then indistinguishable in kind, which is what lets a plugin's output be edited
  and a hand's marks be measured.

  We already have most of the pieces and none of the frame: `waveform`,
  `spectrogram`, `scope`, `bpf` and `pianoroll` are layer types by another name,
  the clip's **edit layer** is a narrow version of the stack, and `Selection`
  (O6) already spans time, value and spectral region - which is exactly what an
  annotation layer is a set of. What is missing is that **the axis belongs to the
  pane and not to the content**: our clip is the box, the axis and the contents at
  once, so two things cannot be laid over one axis without one of them owning it.
  That is the design question this milestone opens, and it is what makes the
  audio editor more than a waveform with a selection.

  **The multitrack editor plays through the server's transport, and computes no
  time at all** *(decided 2026-09-08, taken as the milestone's first leg)*. This
  is not a new decision - `docs/architecture.md`, "Playback time in a session:
  read, never computed", already writes it for the `standalone` host, and
  `docs/schemas.md` already calls `TransportPos` *"the shape a multitrack needs
  - many readers, one time"*. What is new is that the multitrack editor is the
  application that has to use it, and today's example does the opposite: it
  builds a `clausters.seq.Timeline` afresh per pass and runs a **client-side**
  `Playhead` that scans onsets and fires one-shots, so the server never learns
  that there is a piece.

  The machine is already on the wire and unused by this application:

  - The transport **exists on every server**, with no beat grid needed - rolling,
    stopping, saying where the piece is and looping a span are all in samples.
  - `/transport_group` binds the subtree the engine **governs**: `stop` freezes
    it with every node's state intact, `play` thaws it, so resuming *continues*
    rather than restarts. The host binds a group of its own and never the root,
    which would freeze every sound the session has.
  - `/transport_locateSample`, `/transport_locate` and `/transport_loop` are the
    three verbs an editor wants, and the loop's wrap happens **in the engine**,
    on its exact sample, so no client is in the loop and a reader hears no seam.
  - `TransportPos(offset)` is what makes a region a **follower**: a `BufRd` on
    that phase seeks when the transport seeks, loops when it loops and holds
    when it stops, with nothing sent per pass and no position of its own. The
    subtraction is `f64` inside the UGen, so a region reads its own frame 0
    however deep into the piece it sits.
  - The playhead is **read, not anchored**: `positionSample` is published in the
    shared segment, the host's `HeadClock::Piece` already draws from it, and the
    sweep anchor is then simply 0. A stopped transport holds the position so the
    line holds; a locate moves it so the line jumps; a loop wraps it so the line
    wraps.

  **What this dissolves, and it is most of `C54`.** A region becomes a resident
  node with a span rather than an entry in an onset queue, so *"is this under the
  cursor"* is the reader's own arithmetic and no structure has to answer it; the
  position is the engine's anchored `PiecePosition` and no edit can shift it; and
  an edit is a node command on a live node (`/node_set` of the offset and the
  span, `/synth_new`, `/node_free`) that lands wherever the transport is, with
  nothing re-cued and nothing already sounding cut. `clients/python/PLAN.md`'s
  `C54` carries the split and what is left of it.

  **What it does not dissolve**, stated here so it is not discovered: a region of
  **events** has no reader to follow. Notes fire voices, so that half keeps a
  queue - on `/sched_atTransport`, which rides the transport clock and waits out
  a pause - and keeps needing a re-cue, but only on a **locate**, over what is
  live at that position, instead of on every edit. That is the small half of
  `C54` and it is where "what is alive at beat b" is still a question a structure
  has to answer.

  **Two limits that come with a server-wide transport.** There is exactly one per
  server, so two multitrack windows are two pieces and one transport; the host's
  `Host::owns_transport` keeps them from fighting but does not make them two.
  And a locate moves the position, never a node's state - which costs nothing for
  a follower and is decisive for a **generator**, whose position *is* its state:
  a generated region is seekable only once it is rendered, which is what decides
  what may sit on a track and what has to pass through a render first.

  **The three kinds of contents a multitrack holds**, against what the crate
  already has:

  | What the hand places | How it is written | How it sounds |
  |---|---|---|
  | an **audio file** | `Content::Window` over `SegmentSource::Samples`, with the session's `Source` table saying where | a `BufRd` on `TransportPos`, gated to the region's span, `playrate` scaling the phase and the fades an envelope over it |
  | a **sequence of events** (MIDI, notes) | `Content::Window` over `SegmentSource::Node` - a window onto a node this document holds, which is what keeps a cut of notes a window and not a copy | voices fired on `/sched_atTransport`, re-cued on a locate |
  | a **processing chain** | a **GraphDef** named in the track's `config`, plus the bus its output goes to | `/graph_new`, which is already an auto-sorted group with its private buses allocated and wired |

  **The multitrack editor waits on a widget that owns the arrangement**
  *(`clients/gui/PLAN.md`, `G34` — "The multitrack is one widget, and it owns the
  arrangement")*. The host has no multitrack container: `track` and `clip` are
  widgets a script drops into whatever generic container it picked, so the
  structure has no owner and a gesture reports *what the hand did* to whichever
  widget it touched, under one of three tags. `G34` makes `multitrack` a heavy
  widget on the `pianoroll`'s shape — lanes and clips become **data in its
  props**, and it reports the arrangement as it now stands. This application
  cannot be written before that, because there is nothing to write it against;
  and once it is, the multitrack goes through `edit()` and has a history without
  being given one. The decision that shapes it is taken there: **the multitrack
  places, and a clip is entered to edit** — which is the same line these three
  applications are drawn on.

  **First leg landed 2026-09-08: the standalone host binds the piece.**
  `clients/gui/src/host/document/piece.rs` draws a `Multitrack` into `G34`'s
  `multitrack` widget and reads a hand's answer back as this crate's own
  vocabulary — `PlaceRegion` for a move *and* for a track crossing (one verb,
  which is why a cross is not a second mechanism), `TrimRegion` for a width,
  `SetLane` for a box the payload no longer names, `SetTracks` for a strip. The
  tree and the piece are **two structures in one history**, so `Ctrl`+`Z` walks
  both in the order the hand made them. What it fixes is not subtle: a session
  written by the current client carries no general tree, so `--session` had been
  opening every real session as an **empty window**.

  What that leg deliberately does not carry, each written where it is owed: a
  **row is a track**, showing its active lane, and expanding a track to its
  takes is view state `O23`'s `View` has a field for and nothing reads yet; a
  **left trim** reports a `start` the reader drops, because `TrimRegion`'s
  `content` is a whole `Content` and the widget states a frame; and a region
  whose content is notes or a composite draws as a named box, since the widget's
  box is a window onto one buffer (`clients/gui/PLAN.md`, "Found by use").

  **None of the three needs a new mechanism, the chain least of all** *(the user,
  2026-09-08: "el ruteo y las cadenas se hacen con buses y synthdef/faustdef del
  servidor en grupos del servidor, debe ser lo mas simple de todo, ya corre de
  por si, ya esta en el arbol y esta el mecanismo de buses automaticos")*. A
  track's chain is a **GraphDef**: `/def_send graph` declares the members and
  their wiring, `/graph_new` instantiates them as an **auto-sorted group** whose
  execution order follows the bus connections, with the **private buses allocated
  and wired for it**, and `/node_set` on the instance resolves **port names**
  against the graph's surface rather than any member id. That is a mixer strip,
  it is on the wire, it is tested, and both clients build it. A `Track` needs no
  routing fields to use it.

  So the document's share is small and is what it already knows how to carry:
  **which graph, which port values, and the bus the track's output goes to** -
  the `config: Opaque` a track already has, plus a number. `Automation.target`
  then names a **port**, which is exactly the split that field was written on:
  the document knows *which parameter*, never *what the parameter means*. What
  the written model listed as a track's routing (inputs, outputs, sends) is a
  bus each, and what it listed as its kind is a consequence of the graph it
  holds rather than an enumeration the format has to close.

  **A chain a hand adds to and takes from is an auto-sorted group, and that is
  the spelling a track uses** *(the user, 2026-09-08: "graph def puede ser una
  cadena de efectos a la que se le agregan o quitan defs como si fueran
  plugins")*. Yes - and the two spellings differ on exactly that, which is what
  picks between them. A **GraphDef instance** is atomic: the only verbs over one
  are `/graph_new` and `/node_free`, so inserting a plugin means re-sending the
  def and rebuilding the chain, losing every tail and every parameter the hand
  had set. A **plain group with `/group_sortMode 1`** is the same thing built by
  hand - one synth per effect, private buses, and the **same** bus-connection
  DAG recomputing the execution order - except that `/synth_new` and
  `/node_free` add and remove one member while it sounds, and the sort re-orders
  around it with no bookkeeping. So a track's chain is an auto-sorted group;
  a GraphDef is what a *fixed* instrument is, where atomic instantiation and a
  named port surface are the point. Neither needs anything built.

  **And the tracks run in parallel for free** *(the user, 2026-09-08: "incluso
  hay procesamiento en paralelo")*. The same bus-connection analysis that orders
  an auto-sorted group also powers `/group_parallel groupID 1`, which runs a
  group's **independent** children on several cores (`--workers N`),
  bit-identically to the sequential result. A multitrack's tracks are exactly
  that -- independent children of the piece's group, joined only where they meet
  a bus -- so the arrangement's own shape is what the analysis is looking for,
  and the editor gets multicore playback by marking the group rather than by
  anything the application has to build. See `docs/parallel.md`.

  What this leaves genuinely open is narrower than "are plugins in the document":
  it is whether a chain **saved** in a session names a GraphDef by name (and the
  session is unopenable without it) or carries it, which is the same question
  every `Lifetime` on a source already answers for samples.

**None of this starts from nothing on the Rust side.** About 8000 lines of
multitrack behaviour are already implemented in the GUI host - the shared box
geometry (`host/placement.rs`, whose own module doc says a lane and a semitone
row are one structure, which is the observation this turn rests on), the lane and
clip drawing, the gesture machine, the rulers, the layers and the playhead -
plus this crate under them. The turn deleted a Python/TypeScript driver and
deleted nothing in Rust. What has to be read again is not the arithmetic but
**what each of those files takes as its input**: today the structure *is* the
widget tree (`reparent_clip` moves a `Widget` between two `children` vectors),
and under a session those become list operations on a lane. The inventory, file by file, is below - and the rule it ends with holds here: the
host's multitrack code is the most eye-tested part of the project and it is the
half that was right, so when it changes the question is what its input is, never
whether the behaviour was correct.

*The inventory, as read on 2026-09-06:*

| Where | Lines | What it already did |
|---|---|---|
| `host/placement.rs` | 473 | one geometry for every box on a time axis — a note in a roll and a clip on a lane are the same span, grabbed by the same three parts, snapped by the same grid, moved as a block, quantized, hit-tested in a rect |
| `host/graphics/track.rs` | 1702 | the lane and its clips as drawn: the body, the header, the grips, the three clip bodies (waveform, roll, curve) |
| `host/gestures/nav.rs` | 1003 | the lane stack and its bands, `reparent_clip`, edge-scroll while dragging, the vertical view |
| `host/interact/*` | 1071 | the hit-tests, the drag arithmetic, and every edit-back payload a lane or a clip emits |
| `host/ruler.rs` | 2267 | the beats/bars/seconds rulers and the tempo map they read |
| `host/layers.rs`, `scroll.rs`, `play.rs` | 963 | the edit layer of a layered clip, the shared time axis, the playhead |
| `crates/clausters-document` | 5861 | the document, the intent vocabulary and its one applier, the log and its inverses, the typed selection and clipboard, the session format |

What it said had to be read again was the input of each: the structure was the
widget tree everywhere; a clip was three numbers and a label where a region is
more; nothing in the host held a track's identity across a redefine but the
derived id; and the only door into this crate from either client was
`clausters.form`'s.

**What this turn does not settle**, and will not be settled in passing: whether
plugins/processors are in the document at all (the leaf is opaque, and a plugin
is a leaf - but a *send* is routing and routing is authored) - `O24`'s
multitrack answers most of it from the application's side, where the chain turns
out to be a **GraphDef** and the routing a bus, so what is left open is only
whether a saved chain names its graph or carries it; and how a
region's contents are addressed when the source is a function whose arguments
differ per region - which is `O21`(a) asked from the other side, since REAPER's
take is exactly *a reference plus the arguments of this appearance*.


## The projections: one implementation per question, whoever asks it

*(Opened 2026-09-11 with the user, out of a pass in which the same four defects had to be fixed in two languages and two of them turned out never to have crossed at all. The analysis that opened it was in "Future directions" and is here now; that entry is a pointer.)*

Every editable structure in this system is one model with **three endpoints** -- the document that owns it, the host that draws it, the server that sounds it -- and an edit may start at any of them. That is an MVC whose legs are messages, and the crate already decided the discipline for it: **a projection has one implementation, in Rust, and an endpoint only carries.** What is not finished is the set of projections that discipline covers.

**The four questions a domain owes, and the score when this opened.** (1) *Edit* -- a gesture or a script call becomes an `Intent` in the structure's own vocabulary, applied by `intent::apply` alone: **done**, and it is why three clients cannot mean three things by one edit. (2) *Undo* -- the inverse and the coalesce key: **done**, through `domain::coalesce_key`. (3) *View* -- the structure as the props a host draws: **half**, since `multitrack::picture` is shared while the flattening into the wire's flat arrays is per client, and nothing at all is shared for points, samples or events. (4) *Instance* -- what is sounding, and the diff that makes it match: **not started as shared code**, since `nodes::plan` is a pure function of the document and everything after it is state.

So `domain.rs` -- the table that exists because otherwise "every binding would spell every domain's rule again" -- answers one of the four questions it is the natural seat for. The argument that put it there is the same argument for the other three.

**What is actually duplicated, measured rather than assumed** *(over `clients/*/gui/editing`, 4,574 lines of Python against 5,350 of TypeScript)*. The view projection is **12%** of it. The largest single duplicated block is not a projection at all but the **conversation**: `Editor` is 865 lines of Python and 1,023 of TypeScript implementing one protocol -- read a report, check it against the version, turn it into intents, apply, then acknowledge or answer with the picture -- and it is an algorithm, not glue. Then view (561), edit ingestion (525), instance (455). What is irreducibly per-language is the rest: sockets and async, the id/bus/buffer allocators, and writing a payload back onto the client's own objects.

**Authority per endpoint, which is the rule the messages need and nothing stated before.** The document is authoritative for the model; the server for what is *sounding* (node ids, bus runs, buffer contents); the host for **view state alone** -- zoom, scroll, selection, which curve is showing. It is not bookkeeping: the selection defect of 2026-09-11 was a model correction destroying view state, and that is only nameable once each endpoint's own is named. The invariant every projection holds: **a correction carries model and never view.**

**Where the projections live, settled by what the host has to be able to do alone.** It looked like an open question -- the view projection produces the props of the GUI protocol, which is `clients/gui`'s, and this crate does not draw. The requirement settles it: **a standalone host must be able to edit a multitrack with no client in the process**, which `clients/gui/src/host/document.rs` and `--session` already half do, so no projection may sit in a layer above the host. And the objection dissolves on inspection -- **props are JSON, not types**: a client builds a dict of primitives and flat arrays and the host parses a `Map<String, Value>`, so producing them needs no dependency on a renderer. The projections therefore go in **`crates/clausters-editing`**, beside this crate and depending on it and on `clausters-core`, which the host links directly and every client binds through the FFI and wasm doors that already exist.

**What this does not become.** Not an object per structure that both draws and updates. The host draws and must not learn the document; this crate holds the document and must not learn a renderer. What a monolithic DAW gets from putting both on one object is that *one* piece of code owns the transition -- and here that owner is the projection, not the structure.

### The milestones

The order is leaves before trunk: a projection is a function of a structure and moves on its own, while the conversation calls all of them and is smaller once they are gone. Each milestone leaves **both clients green and both twins deleted** -- a projection that exists in Rust *and* in a client is worse than one that exists only in a client, because now they can disagree silently in a third way.

- ✅ **O25 - The crate exists, and one projection crosses it end to end.** *(Closed 2026-09-11.)* `clausters-editing`, with the **view projection for `points`** in it -- the smallest domain there is, one flat array of quadruples -- reached from Python through the C ABI and from the page through wasm, with `points.py`'s and `points.ts`'s own flatteners deleted. The payload is deliberately trivial: what this milestone proves is the *pipeline*, not the projection. **Acceptance:** `PointsView.props` in both clients is a call into the crate and nothing else; the bindings table names the new symbols and `tests/bindings.rs` plus `test_native_parity.py` pass; a curve edited in the Python GUI and the same curve edited in the page produce byte-identical props for the same structure, held by a vector test rather than by reading the two.

  **What landed.** `clausters-editing` with `points::props`, reached as
  `clausters_editing_points_props` over the C ABI (core ABI **v50**) and as
  `pointsProps` over wasm. Both clients' `PointsView` is now a single `drawn`
  call that asks the crate and keeps only what a view keeps -- the axis and span
  in hand -- so `axis()`, `_quads`, `curveAxis`'s caller and the "state no
  duration when there is none" rule stopped existing twice. The parity is
  `clients/web/tests/editing-vectors.json` and `editing-parity.test.ts`, seven
  cases covering a first draw, a held axis, an axis the data outgrew, a curve
  that spans nothing, a ragged tail and an empty curve; it is a file of its own
  rather than a case in an existing one because it is where `O26`-`O29` land.

  **What the small payload taught, and it is the reason to have started here.**
  The axis a curve is drawn against was *already* shared (`curve_axis`, in the
  core) and the two clients still drew from it differently -- one asked
  `max(times)` over a list seeded with zero, the other over a list that might be
  empty, and each assembled the props itself, including whether to state a
  duration at all. **Sharing the rule is not sharing the projection**: what
  crosses has to be the payload an endpoint reads, or the last step is written
  per client and that is the step that carries the decisions.

- ✅ **O26 - The view projection for the rest.** *(Closed 2026-09-11.)* `samples`, `events` and the multitrack -- the last of which is the one with substance (`_lanes`, `_clips`, `_curves`, `_layers`, `_points`, `_bases` and their twins), and the one where `multitrack::picture` stops being half a projection and becomes the whole of one. **Acceptance:** no client shapes a prop payload out of a *document* structure; the props of a session opened in both clients are equal field for field.

  **It was written three times, not twice, and the third was already in Rust.** `clients/gui/src/host/document/piece.rs` builds the same picture for a standalone host -- the same sextuple, the same septuple, the same `frame_at`/`frames_over`/`start x rate`, the same `96.0` -- because a host with no language client still has to show what it holds. So this milestone was less "write Rust" than "stop having three": `clausters_editing::multitrack` holds `lanes`, `clips`, `curves`, `layers`, `points`, `hidden` and `loops`, the host's `shown` keeps only the *binding* (which node each row and box stands for, which is what an edit-back resolves against), and both clients ask the door.

  **What the caller brings is a question, not a table** (`Buffers`): which server buffer a source was read into is held three ways -- a client's table, the host's resolved takes, a JSON map off the wire -- and a trait is what keeps a fourth from being built to satisfy the crate. The table itself is **the instance plan's**, not a second one shaped for drawing: what a box is drawn from and what it is played from are the same samples.

  **A divergence the move surfaced immediately.** A layer's break-points are its *box's* own time and a row's are the timeline's -- `frames_over(base, at)`, not `frame_at(base + at)` -- and the first draft had the crate doing the second. It is the kind of defect that does not fail loudly: both are valid positions, so a clip envelope simply draws where its box is instead of at its start. The Python suite caught it in one test; there is now a crate test that states it, which is the point of the move.

  **Why `samples` and `events` did not move, stated so the scope is not read as an omission.** A samples view's props are the single word `reload` -- the picture *is* the server's buffer, so what corrects it is "read it again". And an events view shapes its payload out of the **client's own objects** (a `Timeline` of `OscItem`/`MidiItem`, read through `_pitch`, `_velocity`, `_label_of`), which is the irreducibly per-language half this whole track leaves in place: a projection is over a *document* structure, and that one is not. What their twins do share is the GuiDef **builders** (`_flat_notes`/`flatNotes` and the rest of `guidef`), which is a larger duplication belonging to a different question than this one.

- ⬜ **The applications after this one are built on it, and it is not the multitrack's** *(the user, 2026-09-11)*. The three classic applications over one document -- the audio editor, the multitrack editor, the score editor -- and whatever opens next are each a structure with the same three endpoints and the same four questions. The multitrack is where this design is being worked out because it is the one with every case in it, **not** because the design is its own: a new editor states its projections in `clausters-editing` beside these, and gets the doors, the parity vectors and the standalone host for nothing. An editor that grows a projection of its own in a client is the defect this track exists to retire, whatever structure it draws.

- ✅ **O27 - The edit ingestion.** *(Closed 2026-09-11.)* A report from the host -- a tag and its values -- becomes payloads in the structure's vocabulary, in the crate, for all four domains. What stays in a client is `project`: writing an applied payload back onto the client's own objects, which is the one thing a language owns. **Acceptance:** `Domain.payload`/`payloads` is a call into the crate in both clients; the gesture vocabularies of the two cannot differ, and a tag the crate does not know is refused identically in both.

  **What landed.** `clausters_editing::{points, samples, events, multitrack}::intake`, behind **one** door -- `clausters_editing_intake` over the C ABI (core ABI **v52**) and `editingIntake` over wasm -- with the vocabulary named as an argument. One door and not four because a host reports every gesture the same way, and because a fifth vocabulary then has nowhere to grow: a client cannot add one without adding it here. A `Domain` now states a **request** (what its vocabulary needs beside the report: the piece, the timeline, an axis, or nothing) and the base class does the rest, so `_placed`, `_curved`, `_strips`, `_rows`, `_groups`, `_bases`, `_notes_now`, `_markers_now`, `_kept`, `quintuples`, `pairs` and both `LABELS` tables stopped existing, in two languages.

  **And it was written three times again, exactly as the view was.** `clients/gui/src/host/document/piece.rs` had its own `read_clips` and `read_lanes` -- the same septuple, the same beat crossing, the same buffer-to-source reverse lookup -- because a standalone host reads a gesture with no client in the process. They are one `piece::read` over the projection now, and the host gained two things by it: **a curve can be dragged in a standalone host** (the `points` tag reached no reader before, though the host has drawn curves since `O26`), and **its mixer can add and remove a track**, which `picture::read_rows` has always said and the host's own reader never did.

  **Three answers off one reading, which is the shape and not a convenience.** `Intake` carries the payloads, the label, and either an `inverse` or a `refusal`. The label travels with the *gesture* rather than the payload because for one vocabulary it is not a function of the payload at all -- both of a roll's lanes state the same whole-list intent, so only the gesture knows whether a hand edited the notes or the markers, and each client held a `_verb` field to remember it between two calls. The `inverse` is there for the one vocabulary whose inverse arrives *with* the gesture (a stroke over samples states what it wrote and the host sends what it replaced beside it), which retired the other held field. And a **refusal** stays distinct from an empty answer: nothing-to-say leaves the picture alone, a refusal makes the host redraw it.

  **The refusal sentence names no language now.** It said `timeline.add(beat, OscItem(addr, ...))` in one client and `new OscItem(addr, ...)` in the other; one string in the crate would have shown a page a script's spelling, so it says *what* to do and not how to type it. A user-facing sentence is part of what an edit is to the person who made it, which is the same argument as the label table.

  **What did not move, stated so the scope is not read as an omission.** The wire's own framing (a `draw` carries its run as a little-endian `f32` blob -- a `memoryview` here, an `ArrayBuffer` there, and neither in a JSON request), and `PointsDomain.state`/`flat`, which cross into the *client's own* `Env` object and are `project`'s half rather than a gesture's.

- ✅ **O28 - The instance: what is sounding, and the diff that makes it match.** The reconciler. `Instance` holds node ids, bus runs, bufnums and the last ports sent, keyed by the document's own ids; `Instance::reconcile(&plan) -> Vec<Op>` is pure -- no socket, no async, no allocation policy -- and the resources come in through a small trait the client implements, which is this design's `HostConfig`. Stating the result as `Op` rather than as bytes is what makes it testable offline, usable by a standalone host with no client in the loop, and identical under NRT. **It wants one server verb it does not have**: a clip changes track by being made again, because a slot instance's wiring is baked into its `out` controls at instantiation -- see `/graph_moveSlot` in `PLAN.md`, "Future directions". Until that exists the rebuild stays and this milestone reproduces it exactly. **Acceptance:** `playback.py` and `playback.ts` are transport and allocation only; the four defects of 2026-09-11 (the stale map, the rebuilt readers, the hand writing a driven port, the clip set across tracks) are regression tests in the crate rather than in either client.

  **What landed.** `clausters_editing::instance`: `Instance` holds what was made of the last plan, `Op` is one thing to do to a server, and `reconcile` answers the difference. It opens no socket, awaits nothing and allocates nothing. Reached as `clausters_editing_instance_*` over the C ABI and as the `Instance` class over wasm (core ABI **v53**) -- a **handle** rather than a function, for the reason `clausters_history_*` is one: it is the only projection with state, and shipping it out and back in on every edit would carry every curve's table twice for nothing.

  **The host config became data, and that is the one place the design as written had to give.** The milestone said the resources would come in through a small trait the client implements. A trait is the right shape *in Rust* and it does not cross a C ABI or a wasm boundary at all -- a client cannot implement one from Python or TypeScript -- so the callback became a **name**: the crate states what it wants made and what to call it, the client makes it and remembers. Same separation, one direction of calls instead of two, and the reconciler stays a function of data rather than of a caller's behaviour, which is what made it testable. The trait shape survives for the one caller that *is* Rust, and `Buffers` in the view projection is the precedent.

  **Handles are how a crate that allocates nothing names what it made.** An op never carries a node id, a bus index or a buffer number; it carries a string minted from the document's own ids, and the client keeps the one table from that to whatever it made. A port that has to name a resource names it the same way (`{"bus": "meterbus:1", "offset": 1}`). That is React's host config with the names changed, and it is what leaves `playback.py` and `playback.ts` with exactly three things: a socket, an allocator, and the table. 497 lines of Python became 310 and 663 of TypeScript became 415, and what went was the diff -- written twice.

  **A handle is stable across a rebuild, and that is a decision with a cost.** The name is what a client's table wants (`clip:3` is "the node that plays region 3", whichever making it is on), so "the node this curve was mapped onto went away" is not expressible by comparing names. A per-region **making** counter is, and it lives apart from the clip's state because the two have different lives: a box dragged to another track leaves its old track's table before it reaches the new one's, so by the time the making matters the state it was in is gone. The first draft compared owners by handle and the regression test for the 2026-09-11 envelope defect caught it immediately, which is what those tests are for.

  **Freeing a group frees what is inside it**, so a `Free` carries the handles that went with it. Without that the client's table would grow by every box a session ever removed -- and a second free would name a node that is already gone.

  **The four defects of 2026-09-11 are crate tests now**: the hand writing a driven port, the clip set across tracks, the rebuilt readers, and the stale map. Each was a client-side unit test in Python with no twin in the page; they are one test each in the crate, and what the clients keep is a test of *their* half -- that an op list becomes the right calls and the right table.

  **A sizing pass must change nothing, and this is the first door where that is not free.** Reconciling is what teaches the instance what is sounding, so the size-then-fill shape had the sizing call do the work and the filling call find nothing left to do. The diff is taken against a copy and the copy adopted in `fill`'s commit closure -- which is what that closure has always been for, and this is the first surface that needed it.

  **`/graph_moveSlot` is still missing and the rebuild is reproduced deliberately.** Until the server can re-parent a slot instance and re-bake its wiring, a clip that changed track is torn down and made again. It is one arm of one match now, with a test stating today's behaviour, so the verb arriving changes one place. **It arrived 2026-09-14**: a clip that changed track is an `Op::Move` (`/graph_moveSlot`), so its readers and the maps on its ports stay; only a source of another width is still made again.

- ✅ **O29 - The conversation.** *(Closed 2026-09-12.)* `Editor`: read a report, check it against the version, turn it into intents, apply, then acknowledge or answer with the picture -- including the minting correction, whose two implementations are where this week's identity defects came from. The largest block and the last, because by then it calls three things that are already shared. **Acceptance:** the host cannot tell which client it is talking to from the message sequence, held by a recorded exchange replayed against both.

  **What landed.** `clausters_editing::conversation`: `Conversation::read` says what kind of turn a message is -- a close, a history step, an edit made against a picture that is gone, or an edit to route -- and `answer` says what to send back (`silent`, `ack` or `push`). Two **pure** doors, `clausters_editing_conversation_read` and `clausters_editing_conversation_answer` over the C ABI, `conversationRead`/`conversationAnswer` over wasm, core ABI **v54**. No handle: the whole state this protocol has is **two integers** -- the floor, and the version the last answered event left behind -- so a client keeps the pair and hands it back, which is cheaper to reason about than a lifetime and is why the instance projection's shape was not copied here.

  **It did not shrink the clients, and that is the finding rather than a disappointment.** The editors went 933 to 927 and 1027 to 1021, and the two `Echo`s *grew* (136 to 144, 154 to 196) because marshalling a request costs lines that an inline `against < floor` does not. The measurement that opened this track -- 865 lines of Python against 1,023 of TypeScript "implementing one protocol" -- counted an editor's whole life: drawing, opening, closing, polling, widget ids, stepping the pile, the selection. The **protocol** inside that is about sixty lines, and those sixty were where every version defect of the week came from. What this milestone removed is not lines, it is the number of *rules* that exist twice, and on that count it took out every one the conversation had. The rest of an editor is calls into a client's own objects, which is the half a language owns and the half that should stay.

  **The envelope crosses and the payload does not.** What a report *means* is `O27`'s and already crosses once, so a drag reporting a thousand boxes costs the decision nothing: nine small fields go in. That is the reason the reading and the decision are separate calls at all, and it is what made moving the conversation affordable.

  **The floor was the defect-prone part and it is one implementation now.** Both clients spelled `against != 0 && against < floor` and both raised the floor on `version != applied`; each was one edit away from disagreeing about which gestures a hand is allowed to make. `Echo.stale` and `Echo.raise_floor` are **deleted** in both, not delegated -- a thin wrapper that restates the rule is the rule written twice.

  **The minting correction stopped striding props.** `clausters_editing_multitrack_names` answers what a piece calls its rows and its boxes, so the view keeps what it was told and compares against that rather than against `props["lanes"][::6]`. `SEXTUPLE` and `SEPTUPLE` stopped existing in both clients, which is the last place a flat prop's shape was restated at a call site.

  **And it found what `O26` had left behind.** The web client still held `laneProps`, `clipProps`, `curveProps`, `layerProps`, `pointProps`, `domainOf`, `ROW_H` and `CURVE_H` -- the whole view projection, defined and called by nobody, because TypeScript does not complain about a dead module-private function and the Python twin had been deleted. Dead code in one client and not the other is a divergence waiting to be revived by the next person who finds it and assumes it is live. Gone, with the four orphaned report wrappers (`multitrackRead`, `multitrackReadPoints`, `multitrackReadRows`, `multitrackPicture`) that `O27` left on the package surface and the Python client never had.

  **The acceptance is a recorded exchange.** `editing-vectors.json` gained ten turns replayed as a fold -- an opening edit, a second gesture inside one round trip, a script's edit that raises the floor, the refusal that follows, an unstated version, an undo addressed to the window, a widget this editor never drew, a malformed event, somebody else's close and this editor's -- plus five answers and the names of three pieces. `applied` is part of the recording rather than bookkeeping around it: it is what turns "the version moved" into "it moved by someone *else*", and the first draft of the exchange left it out and read the lag rule backwards, which the vector caught before either client did.

- ✅ **O30 - The host edits a multitrack alone.** *(Closed 2026-09-14: the standalone host edits and sounds a piece through `O32` and `O33`, and a session it saves opens in either client with no conversion -- read and written back unchanged by both, and loaded into a server by both through `Session.load`, the load the host plans too.)* The requirement that settled where the projections live, taken as a milestone: `clausters-gui` opens a session, edits it and sounds it with no client in the process. It links the same crate rather than growing a fourth implementation, which is the whole of why the order above is worth its cost. **Acceptance:** a multitrack edited and played from the standalone host, and the same session opened afterwards from either client with no conversion.

- ✅ **O31 - A join over fragments mints a source.** *(Closed 2026-09-14. The by-ear half was the user's, in the standalone host, after the fixes of 2026-09-14 in `clients/gui/PLAN.md`, "Found by use"; the reopening half is held by a test in each client that loads a saved join of two takes, reads its samples across the seam and compares `/buffer_parts` with the parts the file states. Its step 6 was found while checking that acceptance. Joins are flat since 2026-09-14 -- a join's segments are never joins -- so "it nests" below is what the format admits and not what an edit makes.)* *(Asked for twice by use: 2026-09-11, a take cut into pieces with edges pulled, shuffled and joined; 2026-09-12, the same thing at its minimum -- one cut, two halves, swapped. `clients/gui/PLAN.md`, "Join over fragments", is the report; this is the answer.)*

  **A region is a window onto one source**, so two fragments in an order their source does not have cannot be said as a region, however exactly they sit on the lane. That is why `j` refuses them and why the refusal is not a rule to relax: relaxed, the box plays the source straight through material the fragments skip and runs off the end into silence, which is what it did before `placement::continues` was added. What is missing is not a placement. It is **the source those fragments would be a window onto** -- so the join makes one, and the joined box is then a plain window like every other box.

  **The source is segments, and the segments are arbitrary**: the same source or several, any valid range of each, in any order, with a fade at each seam. That is what `Body::Segments` already states the primitive is ("joining fragments of two files makes one, and cutting one apart gives back the windows it was made of"), what `/buffer_stitch` already installs, and what `/buffer_parts` already answers with. The user's word for it when the segments were designed is **pseudobuffer**: a thing a reader reads like a buffer, which owns no samples, and which a `PlayBuf` or a `DiskIn` can be pointed at without knowing it is one.

  **The recipe lives in the session's source table, as a `Location`** -- `Location::Segments { parts }`, beside `File` and `Volatile`. That is the decision the rest follows from, and it was taken against the other candidate (a `Body::Segments` node in a new `Multitrack::content`, windowed through `SegmentSource::Node`). Both express the same thing; they differ in how many readers have to learn a second shape.

  - A source table is exactly where this belongs by the crate's own statement: the document says *what plays when* and deliberately does not say where a source's samples are, because inside a running system a source is a server buffer, a mapped file or a rendered result. A source whose samples **are spans of other sources** is that same sentence one level in.
  - Nothing downstream changes shape. `picture` still projects one box with one source; `nodes::plan` still asks the caller's table for a buffer and still plans one reader per channel; the instance, the host's drawing and both clients' textures are untouched. A window onto a node would have made every one of them learn a second case.
  - It **nests** -- a join over joins, which the server already allows four deep -- and it is what `/buffer_parts` hands back, so a join editor reads the recipe out of the server in the terms it sent it.
  - It survives the change this is really waiting on. When a long take stops being a pool buffer and becomes a positioned stream (root `PLAN.md`, "A long take is played out of the pool, and `DiskIn` cannot be positioned"), a part naming a source and a frame range is already the right statement: what moves is how a client *realizes* it, and the document does not move at all. **That is the whole reason the recipe is in the document rather than in a `/buffer_stitch` call the client remembers making.**

  **The minted source reaches a live client through the outcome, not through the file.** A join happens while the piece is open, and the source table is the session's rather than the piece's, so `JoinRegions` carries what it minted -- the new source id, its parts, its shape -- the same way a split's minted name comes back as the id the document gave it. A client applying the outcome learns *there is a source N made of these spans* and stitches it; a client reopening a saved session reads the same statement out of the table. One sentence, two arrivals.

  **What it takes, in order:**

  1. `session::Location::Segments { parts: Vec<session::Part> }`, a part being a `SourceRef` whose `range` is the frames it contributes plus `fade_in`/`fade_out` in frames and an optional channel map (identity when absent, which is the ordinary case). `FORMAT` moves to 2: `Location` is a tagged enum with no untagged arm, so a reader that does not know the variant fails rather than misreads -- which is what the counter is for.
  2. `edit::join_regions` stops refusing a run that does not continue. It mints the source, states the parts from each region's own window in lane order, and writes one region windowing the whole of it. A run that *does* continue keeps today's answer -- one plain window over one source -- because it is cheaper, it is what a plain cut inverts to, and a join that always minted would leave a session full of pseudobuffers that are one span of one take.
  3. The inverse drops the minted source with the regions it restores, so undo leaves no orphan in the table.
  4. `Intake`/`Answer`: the minted source travels in the answer, and both clients learn one verb for "stitch this and tell me the buffer".
  5. The host's `j` stops refusing the reordered run and reports it; `why_not_joined` keeps the three conditions that are still conditions (one lane, a run that touches, a hand holding two).
  6. ✅ **A saved session is loaded by every endpoint** *(found 2026-09-14, checking the acceptance)*. "Reopens from either client" had no verb to stand on: both clients read the file as a document, but only the GUI host could load its sources into a server (`host/document/sources.rs`), so a script loaded its takes by hand and could not load a join at all. The load is `clausters_editing::load` now -- each take read, each join stitched after the takes it is over, a take only a join reads included, every step followed by its buffer's `/done` -- bound as `clausters_editing_load` (core ABI **v60**) and `editingLoad`, and walked by the runner in `Session.load` in both clients and in `clausters-gui --session`. The buffer numbers are the caller's; the host looks for each file first, and a client, which cannot see its server's disk, learns of a missing one from the server's refusal of the read.

  **Acceptance:** a box cut in two, the halves swapped and joined, plays the second half then the first, saves, reopens from either client and from the standalone host, and plays the same; the same gesture over fragments of *two* takes does the same; and `/buffer_parts` on the joined source answers the parts the document states.

  **What it deliberately does not do yet**: a gap between fragments (silence) and an overlap (a mix) are the two cases `/buffer_stitch` does not express, and they wait for the server's half. The refusal for those two stays, and now says which.

- ✅ **O32 - The applications crate: the multitrack application, once, in Rust.** *(Closed 2026-09-14: the by-ear half of its acceptance was checked when `O33` closed, and a join made by a hand plays what it joined since the fixes of 2026-09-14 in `clients/gui/PLAN.md`, "Found by use". The window is held **by role** rather than by id: which id an allocator hands out is each client's own, as step 6 records.)* *(Decided 2026-09-13 by the user, after the by-ear check of the standalone host following `C56` (`clients/python/PLAN.md`): no ruler, no transport buttons, a space bar that logs a roll and sounds nothing, and a join that stitches an empty buffer -- "Yo pasaría todo el ejemplo de Python a Rust. La regla se compone, no es parte del widget multitrack", "el standalone tiene que hacer la composición que hace el cliente Python en el ejemplo edit_multipista".)* This is how `O30` is delivered, and `O30`'s acceptance is this milestone's.

  **What is still written twice after `O25`-`O29` and `C55`/`C56`.** Every projection, the conversation, the applier and the playback control are single. What is not is the **application** that calls them: `MultitrackEditor` (`clients/python/clausters/gui/editing/multitrack.py`, `editor.py`, and its port in `clients/web/src/gui/editing/`) composes the window -- a `timeruler` linked to the multitrack, the row of rewind, play/pause and stop, the clock, the status line -- and wires every event of that window to a verb: a click on the ruler places the mark and cues, the buttons and the space bar drive the transport, undo, redo, save, entering a clip, minting a source for a join. The standalone host does none of it through that code. It has its own partial composition (`clients/gui/src/host/document/tree.rs`, `draw_ruled`, which has no ruler and no buttons), its own owner and its own key bindings (`host/document.rs`, `host/instance.rs`), so a defect fixed in the Python application never reaches it, and every one the by-ear check found had already been fixed there.

  **The shape.** A new crate, `crates/clausters-apps`, where the audio editor and the score editor join the multitrack later. It depends on `clausters-document`, `clausters-editing` and `clausters-core`, and **never on the host**: a client cannot link a renderer, so an application speaks to the host in what the protocol already carries.

  1. **Composition as a GuiDef.** The application answers the window as the same JSON tree a client sends in `/gui_def` -- ruler, multitrack, transport row, clock -- and its corrections as the same props. The host builds and reconciles it by the door every client already uses (`widget::reconcile`), so a standalone host feeds it in-process and draws nothing of its own. The GuiDef builders the application needs are written once here, in Rust.
  2. **Events in, answers out.** The application is a state machine over the host's reports (a tag and its values) that answers **host messages** (definitions, corrections), **steps** (from `PiecePlayback` and the applier), and **document effects** (save, the history). It opens no socket and awaits nothing, like every projection before it.
  3. **Steps name where they go.** A step is addressed to *the server that holds the samples* or *the server that sounds*. For a client these are one server; for the standalone host they are two (the in-process session and the player), so a source minted by a join is stitched where the samples are and attached where the piece sounds.
  4. **Clients bind it.** `MultitrackEditor` in both clients becomes a handle over the crate through the C ABI and wasm doors: it forwards reports, sends the host messages to the host, runs the steps against its server, and keeps what a language owns -- the idiomatic surface, the socket, the awaiting, and writing a payload back onto the client's own objects. The doors are declared in `docs/bindings.md`.
  5. **The host runs it.** `clausters-gui --session` holds the same application; `tree::draw_ruled`, the host's owner and its application key bindings go. What stays in the host is what `What stays out of the crate` already names -- the gesture, hit-testing, the drawing.

  **The reference is `clients/python/examples/editors/edit_multitrack.py`**, read call by call: the window it opens and the responses to its gestures are what the crate reproduces, and the Python application is deleted rather than delegated as each part crosses.

  **Steps, in order** *(decided 2026-09-13 with the user: port the Python client step by step, since it is the one known to work best)*. Each leaves both clients and the standalone host green.

  1. ✅ **The window.** `clausters_apps::multitrack::{window, props}`, bound as `clausters_apps_multitrack_window`/`_props` (core ABI **v56**) and `appsMultitrackWindow`/`appsMultitrackProps`. Both clients' `MultitrackView.build` and `props` call it, and `chrome`, `_cursor`/`cursorOf` and `_meters`/`meterBuses` are gone from both. `clausters-gui --session` opens a piece in the same window, numbering the transport row itself (`Transport::Numbered`); `tree::draw_shown` and `tree::draw_ruled` went, and `tree::draw` is left for a document written before the turn. A script's own `extra` widgets stay the client's to append, because a widget over a live source keeps a binding no JSON carries. *Found on the way*: the web client's window said `layout: "col"` where the Python client's said `flow: "col"`; the crate says `flow`.
  2. ✅ **The conversation and the history.** `clausters_apps::multitrack::editor::MultitrackEditor`: one view's end of the conversation, a gesture read (`intake`) and applied with the inverse read before it lands (`domain::edit`), the position cursor and the selection, a refusal answered with the picture and its reason, an overtaken edit handed the picture back, and the minting correction (`settle`) — every turn an `Outcome`. A handle, `clausters_apps_multitrack_editor_{new,free,call}` (core ABI **v57**, which removes the two stateless v56 doors) and `MultitrackEditorCore` in wasm, with its verbs through one JSON door. Both clients' `MultitrackEditor` hand every message to it and carry out what comes back; `MultitrackDomain.request`/`current`, `MultitrackView.told` and the names comparison went from both. The standalone host holds the same editor in its `Owner` (`open_editor`), and `answer_own`'s piece arms, `Owner::read_piece`/`apply_piece`/`piece_answers`/`read_piece_events` went.

     **The history stayed with the caller, deliberately.** A piece shares one undo order with the boxes entered out of it, and those editors are not applications until step 5, so a turn answers the entry to record and a step of the history comes back through `apply`; the version the host names back is that history's counter and crosses in and out. When the audio editor joins the crate the order can move with it. *Found on the way*: the web client's refusal for an unreachable step said "that edit belongs to a window that is not open", a sentence the Python client had retired with the defect it described; both say "nothing here can put that edit back" now. And two web tests took a `Map` entry for a widget id, which only passed because the old route never asked whose widget it was.
  3. ✅ **Playing, from the window.** The editor owns the transport row's buttons like the piece and its ruler, and a turn names what the transport is asked to do (`TransportVerb`): a click on play/pause or the space bar toggles, stop goes back to the mark, rewind puts the cursor at the top and cues there, and a locate on the ruler cues; the clock's text is the editor's (`clock`). Both clients' buttons lost their `on_click` handlers — the core learns the row's ids once the window has numbered them (`controls`) — and `toggle`/`rewind`/`stop` are the core's verbs, so a script's call and a click are one path. The standalone host answers the row and the space bar through the same editor, sets its clock label every frame the reading changes (`tick_piece_clock`), and its space bar no longer rolls a piece before anything else: over nothing a take answers for it tells the **window** `play`, which reaches a script's editor too.

     **What stayed where it was, stated so it is not read as done.** The steps are not addressed to two servers yet: every step a playback answers goes to the server that sounds, and the one that would go elsewhere — a join's stitch — is step 4's. `roll_piece` and `cue_piece` stay as the host's carriers of a verb, the way a client's `Playback` carries one. And the browser host's space bar still does not reach `play_key`, which the native one does (`host/web/input.rs`) — a divergence between the two fronts, written in `clients/gui/PLAN.md`, "Found by use".
  4. ✅ **A source an edit mints.** A join is stitched where the samples are and attached where the piece sounds. The editor already names the source a turn minted (`Outcome::minted`) and the crate already says what the join is (`clausters_editing::sources::stitch`); what was wrong was *where* the standalone host sent it — to the player, so the join sounded while the session, which owns the takes and is what the picture reads, never had it: an empty box that failed when an edge was pulled. Now a host with two servers makes it in the session and attaches it on the player when the session's `/buffer_stitch` is done (`Host::stitching`, `Host::on_server_reply`), the order a session's open already followed.

     **No addressed steps, and the reason is the topology rather than a shortcut.** Only the standalone host has two servers; a client's samples and its sound are one server, and an attach to the server that already holds a buffer is not a verb. So the addressing is the host's, beside `send_to_player`, and neither client changed. A client's `Buffer.stitch` stays the client's builder for the command, as every OSC command's builder is.
  5. ✅ **Entering a box, as far as the multitrack goes** *(narrowed 2026-09-13 with the user)*. What a box opens as is the piece's question and the editor answers it (`box_contents`, the core's `box` verb): the source its region is a window onto and the title a window over it carries. Both clients' `enter` ask it, and `_region`/`_source_of` and `regionNamed`/`sourceOf` went; each still opens its own `SamplesEditor` over the source's buffer, on the piece's history.

     **What opens is another application's, and it is not here.** The editor for what a box holds is the **audio editor**, which is its own application in this crate — `clients/gui/PLAN.md`, `AP7`, decided the same day — and is not grown inside the multitrack. Until it exists the standalone host logs an entered box and opens nothing, and the undo order stays with the caller: once the audio editor is the crate's too, the history a piece shares with its boxes can move into the crate with it. *(Narrowed 2026-09-14 with the user: `AP7` moves the existing `SamplesEditor` into the crate to work out one history over two applications, and is not the audio editor; entering a box from a piece is not part of it, since the multitrack does not edit samples. The audio editor is a track of its own after `AP8`.)*
  6. ✅ **The clients hold a handle.** `MultitrackEditor` in both clients forwards every message to `MultitrackEditorCore` and carries out what it answers — the history entry, the piece written back, a minted source, the transport verb, an entered box — and holds no composition and no event wiring. What was left over went: `Bridge`'s frame and beat conversions (`frame_at`, `frames_over`, `beat_at`, `frame_in`, `beat_in` and their twins), which nothing called once the picture and the reading were the crate's, and `Sources.width`.

     **The recorded exchange** is `editor_exchange` in `clients/web/tests/editing-vectors.json`, generated by `gen-editing-vectors.py` through the Python client's `MultitrackEditor.apply` and replayed in `editing-parity.test.ts` through the web client's: a box moved, a box split under a name the host minted, the cursor placed on the ruler, play, stop and rewind from the transport row, the space bar, an undo, and an edit made against a picture that is gone. Each turn compares what the client told the host, what it asked the playback to do, and the regions of the piece; widgets are named by role, since which id an allocator hands out is each client's own. The standalone host runs the same editor and is held by its own tests (`the_transport_row_and_the_space_bar_reach_the_editor`, `a_stitch_done_in_the_session_is_attached_once`, and the piece tests `answer_own` already had).

  **Status after the six steps** *(2026-09-13)*. Every step is in; the milestone stays open for the half of its acceptance only a person can check — **by ear, in the standalone host**: the space bar sounds the piece, the ruler places the mark, and a join plays what it joined. Entering a box in the standalone host is not part of it, and no longer waits on `AP7` (see step 5).

  **Acceptance:** `edit_multitrack.py`, the page `edit-multitrack.html` and `clausters-gui --session` open the same window -- the same widget ids -- and answer the same gestures with the same document and the same messages to the server, held by a recorded exchange replayed against the crate; in the standalone host the space bar sounds the piece, the ruler places the mark, and a join plays what it joined; the Python and TypeScript `MultitrackEditor` hold no composition and no event wiring.

- ✅ **O33 - The multitrack application is carried out once, too.** *(Closed 2026-09-13, by ear in the standalone host on the session the multitrack example writes: the space bar and the transport row sound the piece, the ruler's mark cues and stop returns to it, the join a session opens with plays what it joined, a split and its undo and redo answer as a client's editor does. Joins made by a hand still fail in cases of their own -- `clients/gui/PLAN.md`, "Found by use" -- which are not what this milestone was about.)* *(Decided 2026-09-13 by the user, after the by-ear check `O32` was left open for: the standalone host opened the same piece as `clients/python/examples/editors/edit_multitrack.py` and looked the same, and neither the space bar nor the transport row made a sound — "es un claro ejemplo de implementar las cosas dos veces y mal".)*

  **What `O32` did not reach.** The editor's *decisions* are one: the window, what a gesture means, what the transport is asked to do. What **carries them out** is still written per endpoint, and it is exactly where the standalone host fails. The by-ear run's log shows it: every press reached the host's editor and the transport rolled and froze (`the piece is rolling` / `frozen`), and nothing sounded; and opening the session sent a join's `/buffer_stitch` before the reads it is made of had finished (`part 0: buffer 1 is not allocated`), so the join drew empty and its `/buffer_attach` on the player failed.

  **The silence itself had a narrower cause, fixed on its own** *(2026-09-13; `clients/gui/PLAN.md`, "Found by use", "A standalone host stopped hearing its player")*: the thread that carries the player's replies ended 200 ms after boot, so the host's queue waited on a `/done` nobody delivered. It is not a reason to drop this milestone -- a reply path that dies unseen on one endpoint is exactly what one runner fed by one delivery rules out -- but the by-ear check is no longer blocked on it. What still is: the join's stitch sent before its reads.

  **The three places it is written twice, and what each becomes.**

  1. **One step runner.** A playback answers steps — a message, a `/done` the rest waits for, a sync barrier — and each endpoint carries them out its own way: Python's `Playback._run` pairs a send with a blocking request, the web client's awaits, and the host has its own queue with its own reply path (`host/instance.rs`, `Playing::ready`/`reply`). The runner goes into the shared crates as data: steps in, the messages that may go out now, and a reply offered to what the queue is waiting on releasing the rest. Both clients send what it says and hand it their replies; the host drops its queue for the same runner. **A session's open goes through it too**, so a join waits for the reads of the takes it is made of — which is the empty join above — and a step names which server it goes to, for the one endpoint that has two.

     - ✅ **The runner, and the host on it** *(2026-09-13)*. `clausters_editing::run::Runner`: steps pushed for a `Server` (`Sound`, `Samples`), the messages that may leave with their server, and a reply from a server releasing a wait only if that is the server it was sent to (`Released`, `Refused` with the reason, `Unrelated`). One queue across both servers. The host's `Playing` holds one instead of its queue, every reply reaches it with the leg it came in on (`instance::Leg`, native and browser fronts), and a join minted by a hand is pushed as a stitch waiting in the session and an attach behind it on the player -- `Host::stitching` and its `/done` arm are gone.
     - ✅ **A session's open through it** *(2026-09-13)*. `sources::Load::steps` states the load as steps -- each `/buffer_allocRead` and each `/buffer_stitch` followed by the `/done` of that very buffer, in planned order -- and `clausters-gui --session` drives a runner over the in-process session until it is idle (`drive_session`, bounded at 10 s), instead of sending the batch and counting reads. The session the example writes now opens with every source loaded, the join included, and no `/fail`; and it opens at once, where it used to wait out the whole ten seconds: the count took the stitch for a sixth read, and only the five reads answer as reads.
     - ✅ **The clients on it** *(2026-09-13)*. `clausters_editing::run::call_json` is the runner as one JSON door -- `push`, `ready`, `reply`, `idle` -- bound as `clausters_editing_runner_{new,free,call}` (core ABI **v58**) and `StepRunner` in wasm; `apply::steps_from_json` reads back the steps a playback answered. `Playback._run` and the web client's `run` push a playback's answer, send what is ready, send **the awaited message last** as the request that waits on its reply (`request` in both, matched by command), and hand that reply back. Neither pairs a send with the step after it any more, and neither decides which reply releases what.
  2. **One transport.** The piece's playback is configured differently per endpoint: the clients make it at the root with its graph bound to the transport (`bind_transport: true`), and the host makes it inside a group it governs with the binding off (`bind_transport: false`), a group the take monitor shares. The playback is set up one way everywhere — the crate makes its own group bound to the transport and puts the piece's graph in it — and the host's take monitor stops sharing that group.

     - ✅ **One transport** *(2026-09-13; the monitor's place decided with the user)*. `Op::Transport` makes the transport's group at the top and binds it (`instance::TRANSPORT`), `Op::Graph` names the group it goes in, and the piece's graph is made there; a teardown frees that group with everything inside. `Endpoint` keeps only `chunk`: `target` and `bind_transport` are gone, and so is the three-argument `clausters_editing_playback_new` (core ABI **v59**, breaking), with both clients' `PiecePlayback` constructors. `PiecePlayback::group` answers the group. **The server governs one group**, so a take monitor outside it would stop following the transport: the GUI host's monitor makes a group of its own **inside** the piece's (`Host::monitor_group`), and a host with no piece still binds one of its own, the first time the monitor needs it rather than when the player attaches. The monitor's messages go through the runner, behind the piece's steps, so a reader is never sent before its group.
  3. **One event path.** A client's editor receives a `/gui_event` over its socket; the standalone host's editor is reached by an internal path (`window_verb`, `answer_own`, `answer_piece`) that builds the message itself. The host builds the **same** `/gui_event` a client would receive — the same stamp and version — and hands it to the editor through the same delivery a client's editor goes through, so the only difference left is whether a message crossed a socket or stayed in memory.

     - ✅ **One event, whoever receives it** *(2026-09-13)*. `Host::event_message` builds the `/gui_event` both fronts send -- widget, stamp, the version the host is drawing, payload -- and `Host::deliver` hands that same message to what the host owns; a front sends it on only when nothing took it. `answer_piece` passes the message whole to the editor's `event`, where it used to build one of its own with no version, which applied every edit unchecked. `answer_own` is left as the tests' shorthand for building and delivering one. Held by `an_edit_made_against_a_picture_that_is_gone_is_refused_here_too`. *Found on the way*: the window's undo and redo settled with the tree's version in a piece session (`clients/gui/PLAN.md`, "Found by use").
     - ✅ **The recorded exchange replayed against the host** *(2026-09-13)*. `the_recorded_exchange_is_answered_the_same_by_the_host` delivers the nine turns of `editor_exchange` to a standalone host through `Host::deliver` and compares what it tells itself, what it asks of the playback, whether the piece changed and the regions it is left with -- widgets, and the `link` prop that names one, by role. Every answer the host gives itself now goes through one `Host::tell` (corrections drawn, stamp settled), which is what a test can read. **Replaying it found three things the host did not do**, each one a client's behaviour it had never had: it did not ask the editor's `settle` after a changed turn, so a name the host minted was never corrected; it answered the window's undo and redo in the tree's own arm, where a client's editor reads them as a step of the history (walked, then `resync_all`, then `acknowledge` -- `Host::step_piece`); and it took the piece's own counter for the conversation's version, which parts from the history's on the first split. The owner keeps that version apart now (`Owner::conversed`): the version a changed turn answered with, and one more per step.

  **What stays different, and why it is not a fourth.** The standalone host's two servers — a session in the process that owns the samples, a player that sounds — are its topology rather than an implementation of something a client does, and the runner in point 1 is what addresses a step to one of them.

  **Acceptance:** `clients/python/examples/editors/edit_multitrack.py` and `clausters-gui --session clients/python/examples/out/edit_multitrack.json` (the session the example writes) sound the same piece by ear — the space bar, the transport row, the ruler's mark, and the join, which plays what it joined; no endpoint keeps a step queue, a transport binding or an event delivery of its own; the recorded exchange of `O32` step 6 still passes, and the host is held by a replay of the same exchange.

## What stays out of the crate

**The gesture**, in every form - the drag machine, the draw stroke, the lasso, the marquee, hit-testing, the pending overlay's drawing, the visible refusal when a zoom cannot express an edit. Those are the GUI host's and stay in `clients/gui/PLAN.md`'s D and H tracks, which are reformulated as the view halves they always were.

**The wire.** The crate defines intents and outcomes; it does not encode them. OSC framing, `/gui_*` addresses and the transports stay where they are.

**Audio processing.** The crate describes edits; it does not perform DSP. The server owns processing, `clausters-core` owns shared numeric work, and the crate owns neither.

**An interpreter.** A generator is code and the crate never evaluates one. See Future directions for the one case where something might have to.

## Definition of done (per milestone)

The project rule: code plus tests, a clear commit message, this file's checkbox, the developer and user documentation where the change touches them, and a commented example when the feature is user-facing - the example being how new visible behavior is checked by eye. A `docs/decisions.md` entry only where a choice has non-obvious context. **And the packages move together**: a change to the intent vocabulary or the document format closes with its pass over the Python client, the web client and the GUI host in the same commit.

## Future directions (to fold into milestones as they firm up)

Every entry carries a checkbox, and one that converges into numbered milestones leaves this list rather than being ticked here. *(Questions that merely have no answer yet are not here - they are under "Open decisions" above, because a decision waiting to be taken and a capability waiting to be built are different kinds of pending and reading them as one is how the first gets settled by accident.)*

- ✅ **The samples vocabulary is read twice, and a blob is the reason** *(named 2026-09-12, closing the routing above)*. `samples::intake` says what a `sample` or a `draw` report means -- the write, and the run it replaced as its inverse -- and the GUI host's `Owner::read_event`/`read_inverse` say it again, in `Intent::WriteSamples`. The two agree today and neither is long, which is exactly the shape a drift takes.

  What stops it being the same call is the **transport**, not the rule: a stroke's run rides the wire as a little-endian `f32` **blob**, and the crate's door takes JSON numbers. The Python client already pays that conversion (`SamplesDomain.request` reads the blob into numbers before asking the crate), so the host could -- at a JSON array per stroke, on the one editing path where the payload is large by design, and then a parse back into a typed `Intent` to apply it.

  The decision is whether the crate grows a door that takes the samples as samples -- which is what a Rust caller holding a decoded run actually has, and what a wasm one holds too -- or whether the host keeps its own reading and the two are held together by a vector test instead. The second is cheaper and states the risk honestly; the first is the rule this file keeps everywhere else.

  **Taken the first way, 2026-09-12.** `samples::write` is the typed door -- the numbers in, the write out -- and `intake` is that same function with the JSON reading in front of it, so a client and a host reach one rule by the road each of them already has. It speaks `f64` because that is what a report's numbers are once read: the wire's samples are `f32`, every value comes from one, both conversions are exact, and neither door has to know which the other used. The host decodes the blob and asks; nothing turns a stroke into a JSON array to be told what it means.

  **And the host was short two rules, which is the answer to whether this mattered.** The copy it kept recorded an **empty stroke** as an edit (a pile entry that undoes to itself) and accepted an **inverse of a different length** than the write it was meant to undo (an entry that leaves half the edit standing). Both are the crate's rules, both are now the only ones. The third difference was the label: `"edit a sample"` and `"draw"` in the host against `"draw the samples"` in the crate, so one gesture read two ways in the undo stack depending on who owned the document. One verb, one name.

- ✅ **The host's owner reads by domain, and today it names the multitrack** *(found 2026-09-12, in a consistency pass over the GUI host requested by the user)*. `clients/gui/src/host/document.rs` holds two vocabularies in one type. The tree's half is `read_event`, a flat match over `"clip" | "sample" | "mute" | "solo" | "level" | "draw"` that builds `Intent`s; the piece's half is `read_piece_events`, `apply_piece`, `read_clips`, `read_lanes`, `bind_multitrack` and `multitrack()`. The comment on the second says why there are two readers rather than one -- the tree has one verb for a placement and the piece has three -- and that is honest for a host with one application in it.

  `O24` puts three in it. The audio editor arrives asking for `read_samples` and `bind_pane`, the score editor for its own pair, and the type that already carries two vocabularies carries five. The shape that answers this is in the crate already and the host is the one caller that did not take it: `clausters_editing::intake_json` reads a tag against a **domain**, over the four there are, and a tag no domain answers for is refused rather than quietly grown into a fifth vocabulary. `O27` closed by deleting exactly this duplication from `piece.rs`; what stayed is the *routing* above it, still spelled with the multitrack's name in it.

  So: the owner binds a widget to a node **and a domain**, and reads a report by asking that domain -- one door, the applications behind it. What each application then owns is what genuinely differs: which node a gesture addresses and what the picture is redrawn from.

  **Closed 2026-09-12, and the list was two words short.** The dispatch read `tag == "clips" || tag == "lanes"` and the projection answers for **four**: a curve dragged in a host with no client attached reported `points` to a reader with no arm for it, and a `j` reported `join` the same way -- both fell through to the tree's reader and left on the wire, to nobody. So a standalone host drew curves it could not edit and had a join key that did nothing, and neither had ever been noticed, because the path with a client attached is the one that gets used.

  `multitrack::answers` is the vocabulary, declared once where it is implemented, and the host asks it. `multitrack::reading` is the second half: what a report came to, **edits or the reason there are none**, because "the hand changed nothing" and "the piece refused" are the same empty list and opposite things to tell the person who pressed the key. `read` and `intake` are both written on it, so neither door can drop the half the other keeps -- the host was dropping the refusal, which is how the one verb whose answer can be *no* came to be a key that did nothing and said nothing. It now settles the acknowledgement with the reason, which puts the document's own sentence on the window's status bar: the same line a client's `/gui_ack` would have put there, in a host that has no client.

  Held by `the_pieces_own_vocabulary_reaches_the_owner_whole` and `a_piece_that_refuses_a_verb_says_so_on_the_bar`, both against a host drawing a piece.

  **What stayed, and why it is not the same job.** The tree's own reader (`clip`, `mute`, `solo`, `level`) is a vocabulary the crate has no domain for -- it is the description `O21`-`O24` are retiring -- so routing it by domain would mean inventing a fifth domain for something on its way out. The `samples` overlap is real and is filed on its own below.

- ⬜ **A clip's fades are its own property, with session defaults** *(decided 2026-09-14 with the user, after reading what the field's multitracks do: Ardour, Cubase, Pro Tools, REAPER, Logic)*. The common model: a fade belongs to a clip's **edges** and is never its gain envelope; a crossfade is what two regions **overlapping** on one track make, join or no join (Ardour: "region fades *are* crossfades", the region beneath heard through the inverse shape); the shapes are at least equal gain (linear, for correlated material) and equal power (the general case); and the length and curve a new fade takes come from a **configurable default** (Cubase's auto fades, 1-500 ms, per project or per track; REAPER's crossfade length and shape on split; Logic's crossfade time and curve for merge and comping). What follows here:

  1. **A fade is a property of the region.** A region's `fade_in`/`fade_out` -- a length and a shape -- is the one object, drawn on the box and moved by a handle. The `gain` automation stays the mix's and never carries a fade.
  2. **The preset is the default of that property, and it is the session's.** It states what a new fade on a clip starts as -- length in ms (10 ms unless the session says otherwise, since every clip is added with one) and shape, equal power or equal gain among the shapes -- and it is written as defaults for a property rather than as a crossfade setting, so the same mechanism can hold other properties' defaults later. It can be **turned off for the whole session**, and **per clip**, where a clip that turns it off keeps butt edges whatever the default says.
  3. **An overlap is a crossfade made out of the fades the clips already have.** Where one clip overlaps the next on a lane, each **stretches its own fade by the time it overlaps the other**: the ending clip's `fade_out` and the beginning clip's `fade_in` both come to cover the overlap, which is symmetric by construction when two clips overlap. Nothing new is stored -- the crossfade is two region fades adjusted -- and it sounds as two readers summed, like any two regions on a lane. It waits for a lane that holds overlapping regions at all.
  4. **Open: more than two clips overlapping at once.** The overlaps can be of different lengths and nest or chain, so "each fade stretches by its overlap" does not say which fade answers to which overlap. To be settled before a lane holds overlapping regions.

  Not a milestone yet because how a fade *inside* a box is drawn is a picture question the multitrack has not had to answer, and it is worth answering once, for a clip's own fades and for the crossfade an overlap makes of them.

- ⬜ **A join takes its clips' fades, and invents none** *(raised by the user 2026-09-12, on the first working join: "le agregaste un cross fade al join, es algo que esta pero no se ve y deberia, puede ser una opcion de join pero tiene que estar especificada y ser configurable"; decided 2026-09-14, with the entry above)*. `O31` fades every seam **where the material is cut** -- 10 ms, `picture::SEAM`, and nothing where two parts read on from each other -- and neither half was asked for. It is **not seen**: the fade is in the source and a box is drawn as a window onto a source, so the picture shows a join and not the shape of its seams, where a region's own fades are drawn where they are. And it is **not stated**: 10 ms is a number this crate chose in a place with no vocabulary for choosing one.

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

- ⬜ **An interpreter inside a standalone host.** The capability behind the second half of the frozen-generator decision above, which is written to accept it: a host that can re-evaluate a generator (an embedded QuickJS or the like, since a generator is code and code needs an interpreter) becomes a full editor of documents it did not author, rather than one that can only show what was rendered. Two things the decision already fixes and this must not undo - **frozen remains the fallback** for every leaf the interpreter cannot resolve (an unknown kind, a missing script, a language it does not embed), and the interpreter is a **consumer** of the document rather than a part of it, so the crate is unaffected either way. It opens when a real document has to be reopened somewhere Python is not.
- ⬜ **The host keeps the pile and a client reads what changed** *(proposed by the user 2026-09-09, while settling AP5's undo/redo walk)*. Today the pile is the crate's and a client drives it in its own process, so acts four and five of a walk - find the objects holding those structures, hand each its payloads, then redraw - are written once per client. The direction the user named instead: the **host** holds the history, and instead of the client orchestrating callbacks it receives what changed and maps it onto its own objects (an `OscFunc` waiting, and one table from a wire name to a Python object). Two things follow, and both are the argument: the host becomes an editor of a client's objects, and it already holds the data structures a standalone editor needs - which is what `O24`'s three applications over one document are about.

  **It is not hypothetical.** `--session` already does it: `clients/gui/src/host/document.rs` holds a `History` with two structures registered and walks both in the order the hand made them.

  **Its precondition, and it is `O24`'s question rather than a milestone of its own.** The host can only keep the pile of what the host holds. For a session that is the document and the piece; for a bare `edit(curve)` from a script it is nothing - the client sends `points` as props and the host draws them, never knowing there is an `Automation` behind. So this opens exactly when **every editable structure has a form the host holds**, which is the question `O24` opens and this file's "the three applications" section frames.

  **Three costs, named so the decision is taken with them rather than into them** *(weighed with the user, who judged them not acceptable for now)*. An undo stops being synchronous - `editor.undo()` cannot answer whether anything moved until the host has replied, which is a surface break in both clients and in every test that reads it. `Editor` stops being generic - today it edits anything with a `Domain`, including a three-line object in a test, and only what the crate can name would survive. And editing with no window would need a host process, or the pile gets two homes again.

  What was done instead lowered acts two and three (`History::walk`, core ABI v46), which **this reuses rather than replaces**: a client receiving changes from a host still has to route them to its objects by structure, and that is the same routing.

- **A domain answers one question today and owes four** *(proposed by the user 2026-09-11, widened by them the same day from the clip to every structure the three endpoints share)*. **Converged into milestones and left this list**: it is now the section "The projections: one implementation per question, whoever asks it" above, with `O25`-`O30` under it.

  **Where AP7 left it (2026-09-14).** The order is no longer each client's:
  `clausters_apps::editing::Editing` holds the history and seats every editor as
  a member, and both clients and the standalone host walk that one context. A
  step still hands a client payloads to put back on its own objects, so the
  direction above -- a host that holds the pile of a client's objects -- is
  what stays open.

- ⬜ **An application inside another, so an application is also a composed widget** *(named by the user 2026-09-14, while the applications after the multitrack were being written down in `crates/clausters-apps/PLAN.md`)*. The aim is not nesting for its own sake: it is that the simpler editors that become applications -- the notes editor first (`X3`), the points editor if it becomes one (`X4`) -- can be **used as composed widgets**, inside another application or inside a script's own window, the way a `knob` is used today. That is the more general shape, and a window becomes the outermost composition rather than a special case.

  **What exists.** `clausters_apps::editing::Editing` already seats several applications in one undo order, so two applications sharing a history is solved. Entering a box of the multitrack opens the samples editor on the piece's order -- in **another window**. An application's window builder answers a whole window and takes its widget ids from the caller, the multitrack's window is held by role rather than by id (`O32`, step 1), and a script already appends its own widgets to an application's window.

  **What it would need, none of it decided:** an application that answers a **subtree** (a panel) rather than a window; ids a parent hands a child, by role or as a range; the host's reports for a subtree routed to the application that owns each widget, and that application's corrections carried back the same way -- the reports already name the widget, which makes this plausible and nothing more; what a parent and a child share (a navigation group, the playhead, the position cursor) when a roll stands inside a multitrack; focus, and who answers a key when applications are nested, which meets `clients/gui/PLAN.md`'s `G36` (key bindings as configuration) and "The whole interaction vocabulary is provisional…"; and in a client, the events of a subtree carried to the application's core, which is what `MultitrackEditor` does today for a whole window. `Member` is a closed enum, so every application that joins adds its variant.

- ⬜ **Staleness per node rather than per document** *(named while closing O4)*. Today one counter guards everything: an edit is stale if *anything* moved since the picture it was made against, even something in another lane it could not collide with. That is deliberately conservative and it is right for one owner, where the counter moves only for edits the owner itself just applied and a refusal costs one redone gesture. It becomes wrong the moment two hands edit at once, where unrelated work would refuse each other constantly. The refinement is a per-node revision, so an edit is stale only against the node it names - cheap to carry, and it needs the log (O5) to be worth anything, since without an inverse a refused edit is a lost one either way. Opening it early would buy precision nothing can currently observe, which is why it is here and not in a milestone. It shares its seam with the entry below.
- ⬜ **More than one owner of the same document.** Everything here assumes one authoritative owner per resource, which is what makes a version counter enough and what keeps operational transformation and CRDTs out of the design - an order of magnitude of machinery for a problem the system does not have. Two people editing one composition at once would be that problem, and it would be a track of its own rather than a milestone: the point of recording it is that the current design is a deliberate floor, not an oversight, and the version is the seam it would grow from.

## Found by use: the running list of fixes

Every entry is a checkbox, and a fixed one stays with the record of what was wrong.

- ✅ **A join is a flat list of segments, and a segment never names a join** *(decided 2026-09-13 by the user, after joins of joins nested past the server's four levels -- `clients/gui/PLAN.md`, "Found by use")*. The model, in the user's words: a segment is a read-only two-dimensional pointer into a buffer or a file, `{ source, start, length }`, and a join makes a new object that is a list of those; no recursion. So a `Location::Segments` part names a **take**, never another source that is itself segments. `picture::read_join` holds that by construction: a box over a join reads through to the join's parts, cut to what the box shows (`segments_of`), and a part that states no range, or a span past the end of the join, is refused rather than joined around. What a caller has to know for it is the segments of the joins it holds (`Buffers::parts`); the multitrack editor learns them from the joins it mints, since a join is never edited. A session saved before this may still hold a join over a join, which is read one level through and not repaired.

- ✅ **After a trim, a window's duration no longer says what its box plays** *(decided with the user 2026-09-13; see the note at its end)* *(found 2026-09-13, diagnosing a join that came out empty -- `clients/gui/PLAN.md`, "Found by use", "A join over more than two segments at once fails")*. `picture::read` answers a trim by sliding `window.start` and leaving `window.duration` as the piece had it, on the grounds that a flat report says how long a box is and not how much of the source is behind it. So after a left-hand trim, or on the left half of a split, a window claims more of its source than the box plays -- even past the end of the take. Nothing heard reads the duration (`nodes::plan` plays a box by its position, its length and its window's start) and nothing drawn does, which is why it stayed invisible; `read_join` did, and asked the server for samples that were not there. The join now reads what the box shows. What is still open is **what the field means**: either a trim keeps it equal to what the box shows (and a saved piece stops carrying a number that lies), or it is stated as meaningful only for a box that wraps, where a window shorter than its box is exactly what looping says. The first changes what an edit writes into the format, so it is a decision, not a fix.

  **Decided: neither.** The user's model: a region is a view onto its **whole** source, so a trim hides and pulling the edge back shows (and sounds) again what it hid; keeping the duration equal to what the box shows would throw that away. A join is the more specific case: it turns boxes, trimmed or not, into one box whose source is exactly the sum of its segments, so its edges cannot be pulled past them. So `SegmentRef::duration`, in a region's window, is **how much of the source the window reaches** -- the whole take, or the sum of a join's segments -- and never what the box shows; a trim or a split moves `start`, position and length and leaves it alone, and `start + duration` is not the end of anything that plays. That is what trims already wrote; what did not follow it was `picture::read` minting a box a hand made with what the box showed, and it now writes the source's length where the caller knows it (`reach_of`, from the table's frames). The total length of a take is the **source's** fact and lives in the source table (`Source.frames`), not repeated in each segment -- a segment stays `{source, range}`, read-only, and finds its take's length by id, which is also what a file read with `DiskIn` will need, with no buffer to ask. A standalone host now writes that length down once its takes are mapped (`Owner::learn_lengths`), into both its take table and the session it saves; a client states it with `Source.shaped`, as the multitrack example does. Held by `a_new_box_windows_its_whole_source`.

- ✅ **A second join left an empty box, because a source the piece stopped naming is still a source** *(found 2026-09-12 by the user: "cortando varios segmentos, redimensionandolos, barajandolos y seleccionandolos todos, al hacer join queda un clip vacio"; fixed the same day)*. `fresh_source` minted off the **piece** alone — the largest source id any region windows, plus one — which is how every other id here is minted and is wrong for this one. A source stops being named by the piece the moment nothing windows it (an undo, a box deleted, a joined box cut back up) while the client still holds the buffer it made for it. So the next join was handed an id that already had samples behind it: the client saw an id it knew, made nothing, and the box became a window onto **the previous join** — or onto a buffer that had since been freed, which is the empty box.

  The piece cannot see the table, so the table says: `Buffers::taken` is the question "which sources do you hold samples for", defaulted to none so a caller with no table is still one, and the mint clears the piece **and** it. The user put the rule in one line while it was being fixed — *"los clips con join deberian crear nuevas instancias de segments, la implementacion asi deberia ser simple"* — and that is what makes it simple: a join never reuses, so nothing has to reason about whether two joins are the same join. A **redo** naming the same id is not reuse; it is the same edit.

  **The same pass closed the other half of the shape.** `read_join` *skipped* a name it could not resolve and joined the rest, which would leave that box where it was, under the box now spanning over it. It refuses instead. Every bug this seam has produced has been a verb quietly acting on less than it was given.

- ✅ **A box that went took its envelope's map with it, and the reconciler named it anyway** *(found 2026-09-12 by the user, deleting a box in `edit_multitrack` the first time the instance projection met a hand)*. `reap_curves` emitted an `Unmap` for the port a deleted clip's curve drove -- after the pass that freed that clip. Freeing a node takes its map with it, so the message was about a node the server no longer had; worse, it named a **handle** whose table entry went with the free, which in Python raised a `KeyError` and in the page would have resolved to `0` and sent `/graph_map` to the **root group**.

  Two halves. The crate does not emit it: a curve remembers whether its owner is a box or a track and which id, so the reaper asks whether the owner is still there. What the curve holds of its own -- its node, its buffer, its bus -- is given back either way, since those are the instance's and outlive whatever port they drove. And both clients now answer an impossible handle the same way, by skipping: one raising while the other addressed node zero is the divergence this track exists to retire.

  It is the argument for handles rather than against them. A node id would have been a number the server quietly ignored or, worse, one it did not; a handle with nothing behind it is a question with no answer, which is a thing a client can *see*.

- ✅ **A break-point compared by how its data was spelled, so a page saw an edit where a script saw none** *(found 2026-09-11 by the parity vectors of `O27`, the first time the same report went through both doors and the answers were put side by side)*. `read_points` asked `held.points != curve.points`, which is derived equality over `Opaque(Value)` -- and a JSON number compares by **representation**: JavaScript writes `0.0` as `0` and Python writes it as `0.0`, so a curve reported back exactly as it was drawn came out as a `SetAutomation` in the page and as nothing at all in a script. Nothing failed loudly: the page simply recorded an undo entry per redraw of an untouched curve.

  It is worth reading as a class rather than as a bug. A point's `data` is **opaque on purpose** -- the document carries it and never reads it, which is what lets a client keep a shape there -- and opaque data travels through whichever serializer the endpoint has. So any comparison of it has to be by what it *says*. Fixed with `same_points`/`same_json` beside `read_points`, numeric-aware and otherwise the equality it always was, with a test that states the rule.

  **And it is the argument for the vectors themselves.** Two clients disagreeing about whether a gesture was an edit is not a build failure, not a type error and not a lint; it took generating the same question from one client and asking it from the other.

- ✅ **A window could only be onto samples, so a cut over notes was a copy**
  *(found 2026-09-03 by the user, on the split over notes that shipped the same
  day: "cuando se hace un split de un pianoroll, sigue siendo una ventana a los
  datos originales?" -- it was, until it was written down; fixed the same day)*.
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

  **What an eye pass measured, 2026-08-17** *(reopening `composer.py`'s four-lane piece)*: the take clips keep their waveform and the automation clip its curve, and **both roll bodies are gone** — the melody's because a `Track` comes back as an `Aggregate` of clangs (no aggregate is a roll any more), the pattern lane's because a frozen generator has no rendering to draw. The user reads it as "el pianoroll no está más", which is the same defect at the size a real piece shows it.

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

- ⬜ **A selection's mapping states one tempo, so a selection cannot cross a
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
