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

  **Superseded for the audio editor on 2026-09-22** (`crates/clausters-apps/PLAN.md`, `X1`, "The design: the history holds references to immutable takes, never samples"): a take a history entry names is immutable, and a stroke writes a new take the size of its span rather than writing in place. The two arguments this rested on -- a per-block copy for a short stroke, and a block sequence that would have to be flattened to be heard -- no longer hold now that joins are played as they are.

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
  `crates/clausters-gui/PLAN.md` with the record of what was seen)*. What it found: a
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

## Closed tracks

The tracks whose every milestone shipped are in [`docs/history/plans/document.md`](../../docs/history/plans/document.md),
moved there whole; the plan keeps what is open. The ticked entries of "Found by
use" and "Future directions" are at the end of the same file. The tracks are,
by their headings:

- The milestones
- The notation mode: a sequence that is written, not only played

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
  the application scope's `AP5` was reaching for** (`crates/clausters-gui/PLAN.md`), and it lands here because
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
  looked up **by id**. Every field is one the host already holds or the `AP5` list (`crates/clausters-gui/PLAN.md`) already named; nothing was invented to fill a shape.

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
    `detail` that pointed at one. That is `AP3`'s lesson (`crates/clausters-gui/PLAN.md`) in this structure's
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
  it instead was `AP5`'s own item (4) (`crates/clausters-gui/PLAN.md`): retiring the redraw difference takes
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

  **The pane owns the axis** *(decided with the user 2026-10-16; X13 in the
  applications plan)*. The host's `pane` container carries the existing axis
  props and layers existing signal and curve bodies. Horizontal navigation,
  selection and playback position belong to its timeline group; a body's
  vertical scale remains its own. The active edit body is named by its role
  through `layer`, and `hidden` excludes bodies from drawing and editing. The
  signal's visual stack stays inside the take body. An ordinary layout panel
  remains a layout: it has no measuring axis to own or edit routing to resolve.

  **A take holds the same automation as a region** *(X14, 2026-10-16)*.
  `parts::Take` carries `Automation<Second>` beside its parts, with the same
  SetAutomation intent. Its history snapshots include curves and source spans,
  never samples; applying gain writes a new take through existing server gain
  operations. The curve stays on the take's time axis across sample edits.
  Preview processing remains separate work in the applications plan.

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
  | a **sequence of events** (MIDI, notes) | `Content::Window` over `SegmentSource::Node` - a window onto a node this document holds, which is what keeps a cut of notes a window and not a copy | an **event lane** on the transport (`PLAN.md`, `T8`): the notes as data the server feeds onto the transport's queue ahead of its position, so a locate, a loop and a stop are the transport's and no client re-cues *(was "voices fired on `/sched_atTransport`, re-cued on a locate", corrected 2026-09-28: that is the transport's work done a second time outside it)* |
  | a **processing chain** | a **GraphDef** named in the track's `config`, plus the bus its output goes to | `/graph_new`, which is already an auto-sorted group with its private buses allocated and wired |

  **The multitrack editor waits on a widget that owns the arrangement**
  *(`crates/clausters-gui/PLAN.md`, `G34` — "The multitrack is one widget, and it owns the
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
  `crates/clausters-gui/src/host/document/piece.rs` draws a `Multitrack` into `G34`'s
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
  box is a window onto one buffer (`crates/clausters-gui/PLAN.md`, "Found by use").

  **None of the three needs a new mechanism, the chain least of all** *(the user,
  2026-09-08: "routing and chains are done with buses and the server's synthdefs/faustdefs in the server's groups; it should be the simplest thing of all, it already runs by itself, it is already in the tree and the automatic bus mechanism is there")*. A
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
  the spelling a track uses** *(the user, 2026-09-08: "a graph def can be an
  effect chain that defs are added to or removed from as if they were
  plugins")*. Yes - and the two spellings differ on exactly that, which is what
  picks between them. A **GraphDef instance** is atomic: the only verbs over one
  are `/graph_new` and `/node_free`, so inserting a plugin means re-sending the
  def and rebuilding the chain, losing every tail and every parameter the hand
  had set. A **plain group with `/group_sortMode 1`** is the same thing built by
  hand - one synth per effect, private buses, and the **same** bus-connection
  DAG recomputing the execution order - except that `/synth_new` and
  `/node_free` add and remove one member while it sounds, and the sort re-orders
  around it with no bookkeeping. So a track's chain is an auto-sorted group; a
  GraphDef is what a *fixed* instrument is, where atomic instantiation and a
  named port surface are the point. Neither needs anything built.

  **And the tracks run in parallel for free** *(the user, 2026-09-08: "there is
  even parallel processing")*. The same bus-connection analysis that orders an
  auto-sorted group also powers `/group_parallel groupID 1`, which runs a
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

**Where the projections live, settled by what the host has to be able to do alone.** It looked like an open question -- the view projection produces the props of the GUI protocol, which is `crates/clausters-gui`'s, and this crate does not draw. The requirement settles it: **a standalone host must be able to edit a multitrack with no client in the process**, which `crates/clausters-gui/src/host/document.rs` and `--session` already half do, so no projection may sit in a layer above the host. And the objection dissolves on inspection -- **props are JSON, not types**: a client builds a dict of primitives and flat arrays and the host parses a `Map<String, Value>`, so producing them needs no dependency on a renderer. The projections therefore go in **`crates/clausters-editing`**, beside this crate and depending on it and on `clausters-core`, which the host links directly and every client binds through the FFI and wasm doors that already exist.

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

  **It was written three times, not twice, and the third was already in Rust.** `crates/clausters-gui/src/host/document/piece.rs` builds the same picture for a standalone host -- the same sextuple, the same septuple, the same `frame_at`/`frames_over`/`start x rate`, the same `96.0` -- because a host with no language client still has to show what it holds. So this milestone was less "write Rust" than "stop having three": `clausters_editing::multitrack` holds `lanes`, `clips`, `curves`, `layers`, `points`, `hidden` and `loops`, the host's `shown` keeps only the *binding* (which node each row and box stands for, which is what an edit-back resolves against), and both clients ask the door.

  **What the caller brings is a question, not a table** (`Buffers`): which server buffer a source was read into is held three ways -- a client's table, the host's resolved takes, a JSON map off the wire -- and a trait is what keeps a fourth from being built to satisfy the crate. The table itself is **the instance plan's**, not a second one shaped for drawing: what a box is drawn from and what it is played from are the same samples.

  **A divergence the move surfaced immediately.** A layer's break-points are its *box's* own time and a row's are the timeline's -- `frames_over(base, at)`, not `frame_at(base + at)` -- and the first draft had the crate doing the second. It is the kind of defect that does not fail loudly: both are valid positions, so a clip envelope simply draws where its box is instead of at its start. The Python suite caught it in one test; there is now a crate test that states it, which is the point of the move.

  **Why `samples` and `events` did not move, stated so the scope is not read as an omission.** A samples view's props are the single word `reload` -- the picture *is* the server's buffer, so what corrects it is "read it again". And an events view shapes its payload out of the **client's own objects** (a `Timeline` of `OscItem`/`MidiItem`, read through `_pitch`, `_velocity`, `_label_of`), which is the irreducibly per-language half this whole track leaves in place: a projection is over a *document* structure, and that one is not. What their twins do share is the GuiDef **builders** (`_flat_notes`/`flatNotes` and the rest of `guidef`), which is a larger duplication belonging to a different question than this one.

- ⬜ **The applications after this one are built on it, and it is not the multitrack's** *(the user, 2026-09-11)*. The three classic applications over one document -- the audio editor, the multitrack editor, the score editor -- and whatever opens next are each a structure with the same three endpoints and the same four questions. The multitrack is where this design is being worked out because it is the one with every case in it, **not** because the design is its own: a new editor states its projections in `clausters-editing` beside these, and gets the doors, the parity vectors and the standalone host for nothing. An editor that grows a projection of its own in a client is the defect this track exists to retire, whatever structure it draws.

- ✅ **O27 - The edit ingestion.** *(Closed 2026-09-11.)* A report from the host -- a tag and its values -- becomes payloads in the structure's vocabulary, in the crate, for all four domains. What stays in a client is `project`: writing an applied payload back onto the client's own objects, which is the one thing a language owns. **Acceptance:** `Domain.payload`/`payloads` is a call into the crate in both clients; the gesture vocabularies of the two cannot differ, and a tag the crate does not know is refused identically in both.

  **What landed.** `clausters_editing::{points, samples, events, multitrack}::intake`, behind **one** door -- `clausters_editing_intake` over the C ABI (core ABI **v52**) and `editingIntake` over wasm -- with the vocabulary named as an argument. One door and not four because a host reports every gesture the same way, and because a fifth vocabulary then has nowhere to grow: a client cannot add one without adding it here. A `Domain` now states a **request** (what its vocabulary needs beside the report: the piece, the timeline, an axis, or nothing) and the base class does the rest, so `_placed`, `_curved`, `_strips`, `_rows`, `_groups`, `_bases`, `_notes_now`, `_markers_now`, `_kept`, `quintuples`, `pairs` and both `LABELS` tables stopped existing, in two languages.

  **And it was written three times again, exactly as the view was.** `crates/clausters-gui/src/host/document/piece.rs` had its own `read_clips` and `read_lanes` -- the same septuple, the same beat crossing, the same buffer-to-source reverse lookup -- because a standalone host reads a gesture with no client in the process. They are one `piece::read` over the projection now, and the host gained two things by it: **a curve can be dragged in a standalone host** (the `points` tag reached no reader before, though the host has drawn curves since `O26`), and **its mixer can add and remove a track**, which `picture::read_rows` has always said and the host's own reader never did.

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

- ✅ **O31 - A join over fragments mints a source.** *(Closed 2026-09-14. The by-ear half was the user's, in the standalone host, after the fixes of 2026-09-14 in `crates/clausters-gui/PLAN.md`, "Found by use"; the reopening half is held by a test in each client that loads a saved join of two takes, reads its samples across the seam and compares `/buffer_parts` with the parts the file states. Its step 6 was found while checking that acceptance. Joins are flat since 2026-09-14 -- a join's segments are never joins -- so "it nests" below is what the format admits and not what an edit makes.)* *(Asked for twice by use: 2026-09-11, a take cut into pieces with edges pulled, shuffled and joined; 2026-09-12, the same thing at its minimum -- one cut, two halves, swapped. `crates/clausters-gui/PLAN.md`, "Join over fragments", is the report; this is the answer.)*

  **A region is a window onto one source**, so two fragments in an order their source does not have cannot be said as a region, however exactly they sit on the lane. That is why `j` refuses them and why the refusal is not a rule to relax: relaxed, the box plays the source straight through material the fragments skip and runs off the end into silence, which is what it did before `placement::continues` was added. What is missing is not a placement. It is **the source those fragments would be a window onto** -- so the join makes one, and the joined box is then a plain window like every other box.

  **The source is segments, and the segments are arbitrary**: the same source or several, any valid range of each, in any order, with a fade at each seam. That is what `Body::Segments` already states the primitive is ("joining fragments of two files makes one, and cutting one apart gives back the windows it was made of"), what `/buffer_stitch` already installs, and what `/buffer_parts` already answers with. The user's word for it when the segments were designed is **pseudobuffer**: a thing a reader reads like a buffer, which owns no samples, and which a `PlayBuf` or a `DiskIn` can be pointed at without knowing it is one.

  **The recipe lives in the session's source table, as a `Location`** -- `Location::Segments { parts }`, beside `File` and `Volatile`. That is the decision the rest follows from, and it was taken against the other candidate (a `Body::Segments` node in a new `Multitrack::content`, windowed through `SegmentSource::Node`). Both express the same thing; they differ in how many readers have to learn a second shape.

  - A source table is exactly where this belongs by the crate's own statement: the document says *what plays when* and deliberately does not say where a source's samples are, because inside a running system a source is a server buffer, a mapped file or a rendered result. A source whose samples **are spans of other sources** is that same sentence one level in.
  - Nothing downstream changes shape. `picture` still projects one box with one source; `nodes::plan` still asks the caller's table for a buffer and still plans one reader per channel; the instance, the host's drawing and both clients' textures are untouched. A window onto a node would have made every one of them learn a second case.
  - It **nests** -- a join over joins, which the server already allows four deep -- and it is what `/buffer_parts` hands back, so a join editor reads the recipe out of the server in the terms it sent it.
  - It survives the change this is really waiting on. When a long take stops being a pool buffer and becomes a positioned stream (`crates/clausters-server/PLAN.md`, "A long take is played out of the pool, and `DiskIn` cannot be positioned"), a part naming a source and a frame range is already the right statement: what moves is how a client *realizes* it, and the document does not move at all. **That is the whole reason the recipe is in the document rather than in a `/buffer_stitch` call the client remembers making.**

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

- ✅ **O32 - The applications crate: the multitrack application, once, in Rust.** *(Closed 2026-09-14: the by-ear half of its acceptance was checked when `O33` closed, and a join made by a hand plays what it joined since the fixes of 2026-09-14 in `crates/clausters-gui/PLAN.md`, "Found by use". The window is held **by role** rather than by id: which id an allocator hands out is each client's own, as step 6 records.)* *(Decided 2026-09-13 by the user, after the by-ear check of the standalone host following `C56` (`clients/python/PLAN.md`): no ruler, no transport buttons, a space bar that logs a roll and sounds nothing, and a join that stitches an empty buffer -- "I would move the whole Python example to Rust. The rule is composed, it is not part of the multitrack widget", "the standalone has to do the composition the Python client does in the edit_multitrack example".)* This is how `O30` is delivered, and `O30`'s acceptance is this milestone's.

  **What is still written twice after `O25`-`O29` and `C55`/`C56`.** Every projection, the conversation, the applier and the playback control are single. What is not is the **application** that calls them: `MultitrackEditor` (`clients/python/clausters/gui/editing/multitrack.py`, `editor.py`, and its port in `clients/web/src/gui/editing/`) composes the window -- a `timeruler` linked to the multitrack, the row of rewind, play/pause and stop, the clock, the status line -- and wires every event of that window to a verb: a click on the ruler places the mark and cues, the buttons and the space bar drive the transport, undo, redo, save, entering a clip, minting a source for a join. The standalone host does none of it through that code. It has its own partial composition (`crates/clausters-gui/src/host/document/tree.rs`, `draw_ruled`, which has no ruler and no buttons), its own owner and its own key bindings (`host/document.rs`, `host/instance.rs`), so a defect fixed in the Python application never reaches it, and every one the by-ear check found had already been fixed there.

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

     **What stayed where it was, stated so it is not read as done.** The steps are not addressed to two servers yet: every step a playback answers goes to the server that sounds, and the one that would go elsewhere — a join's stitch — is step 4's. `roll_piece` and `cue_piece` stay as the host's carriers of a verb, the way a client's `Playback` carries one. And the browser host's space bar still does not reach `play_key`, which the native one does (`host/web/input.rs`) — a divergence between the two fronts, written in `crates/clausters-gui/PLAN.md`, "Found by use".
  4. ✅ **A source an edit mints.** A join is stitched where the samples are and attached where the piece sounds. The editor already names the source a turn minted (`Outcome::minted`) and the crate already says what the join is (`clausters_editing::sources::stitch`); what was wrong was *where* the standalone host sent it — to the player, so the join sounded while the session, which owns the takes and is what the picture reads, never had it: an empty box that failed when an edge was pulled. Now a host with two servers makes it in the session and attaches it on the player when the session's `/buffer_stitch` is done (`Host::stitching`, `Host::on_server_reply`), the order a session's open already followed.

     **No addressed steps, and the reason is the topology rather than a shortcut.** Only the standalone host has two servers; a client's samples and its sound are one server, and an attach to the server that already holds a buffer is not a verb. So the addressing is the host's, beside `send_to_player`, and neither client changed. A client's `Buffer.stitch` stays the client's builder for the command, as every OSC command's builder is.
  5. ✅ **Entering a box, as far as the multitrack goes** *(narrowed 2026-09-13 with the user)*. What a box opens as is the piece's question and the editor answers it (`box_contents`, the core's `box` verb): the source its region is a window onto and the title a window over it carries. Both clients' `enter` ask it, and `_region`/`_source_of` and `regionNamed`/`sourceOf` went; each still opens its own `SamplesEditor` over the source's buffer, on the piece's history.

     **Withdrawn 2026-09-23 with the user**: the multitrack edits non-destructively and opens no editor over what a box holds -- the audio editor is a separate application, with a history of its own -- so `enter`, `box_contents`, the `box` verb, both clients' `enter` and the host's double click on a box went (`crates/clausters-apps/PLAN.md`, `X1`).

     **What opens is another application's, and it is not here.** The editor for what a box holds is the **audio editor**, which is its own application in this crate — `crates/clausters-gui/PLAN.md`, `AP7`, decided the same day — and is not grown inside the multitrack. Until it exists the standalone host logs an entered box and opens nothing, and the undo order stays with the caller: once the audio editor is the crate's too, the history a piece shares with its boxes can move into the crate with it. *(Narrowed 2026-09-14 with the user: `AP7` moves the existing `SamplesEditor` into the crate to work out one history over two applications, and is not the audio editor; entering a box from a piece is not part of it, since the multitrack does not edit samples. The audio editor is a track of its own after `AP8`.)*
  6. ✅ **The clients hold a handle.** `MultitrackEditor` in both clients forwards every message to `MultitrackEditorCore` and carries out what it answers — the history entry, the piece written back, a minted source, the transport verb, an entered box — and holds no composition and no event wiring. What was left over went: `Bridge`'s frame and beat conversions (`frame_at`, `frames_over`, `beat_at`, `frame_in`, `beat_in` and their twins), which nothing called once the picture and the reading were the crate's, and `Sources.width`.

     **The recorded exchange** is `editor_exchange` in `clients/web/tests/editing-vectors.json`, generated by `gen-editing-vectors.py` through the Python client's `MultitrackEditor.apply` and replayed in `editing-parity.test.ts` through the web client's: a box moved, a box split under a name the host minted, the cursor placed on the ruler, play, stop and rewind from the transport row, the space bar, an undo, and an edit made against a picture that is gone. Each turn compares what the client told the host, what it asked the playback to do, and the regions of the piece; widgets are named by role, since which id an allocator hands out is each client's own. The standalone host runs the same editor and is held by its own tests (`the_transport_row_and_the_space_bar_reach_the_editor`, `a_stitch_done_in_the_session_is_attached_once`, and the piece tests `answer_own` already had).

  **Status after the six steps** *(2026-09-13)*. Every step is in; the milestone stays open for the half of its acceptance only a person can check — **by ear, in the standalone host**: the space bar sounds the piece, the ruler places the mark, and a join plays what it joined. Entering a box in the standalone host is not part of it, and no longer waits on `AP7` (see step 5).

  **Acceptance:** `edit_multitrack.py`, the page `edit-multitrack.html` and `clausters-gui --session` open the same window -- the same widget ids -- and answer the same gestures with the same document and the same messages to the server, held by a recorded exchange replayed against the crate; in the standalone host the space bar sounds the piece, the ruler places the mark, and a join plays what it joined; the Python and TypeScript `MultitrackEditor` hold no composition and no event wiring.

- ✅ **O33 - The multitrack application is carried out once, too.** *(Closed 2026-09-13, by ear in the standalone host on the session the multitrack example writes: the space bar and the transport row sound the piece, the ruler's mark cues and stop returns to it, the join a session opens with plays what it joined, a split and its undo and redo answer as a client's editor does. Joins made by a hand still fail in cases of their own -- `crates/clausters-gui/PLAN.md`, "Found by use" -- which are not what this milestone was about.)* *(Decided 2026-09-13 by the user, after the by-ear check `O32` was left open for: the standalone host opened the same piece as `clients/python/examples/editors/edit_multitrack.py` and looked the same, and neither the space bar nor the transport row made a sound — "it is a clear example of implementing things twice, and badly".)*

  **What `O32` did not reach.** The editor's *decisions* are one: the window, what a gesture means, what the transport is asked to do. What **carries them out** is still written per endpoint, and it is exactly where the standalone host fails. The by-ear run's log shows it: every press reached the host's editor and the transport rolled and froze (`the piece is rolling` / `frozen`), and nothing sounded; and opening the session sent a join's `/buffer_stitch` before the reads it is made of had finished (`part 0: buffer 1 is not allocated`), so the join drew empty and its `/buffer_attach` on the player failed.

  **The silence itself had a narrower cause, fixed on its own** *(2026-09-13; `crates/clausters-gui/PLAN.md`, "Found by use", "A standalone host stopped hearing its player")*: the thread that carries the player's replies ended 200 ms after boot, so the host's queue waited on a `/done` nobody delivered. It is not a reason to drop this milestone -- a reply path that dies unseen on one endpoint is exactly what one runner fed by one delivery rules out -- but the by-ear check is no longer blocked on it. What still is: the join's stitch sent before its reads.

  **The three places it is written twice, and what each becomes.**

  1. **One step runner.** A playback answers steps — a message, a `/done` the rest waits for, a sync barrier — and each endpoint carries them out its own way: Python's `Playback._run` pairs a send with a blocking request, the web client's awaits, and the host has its own queue with its own reply path (`host/instance.rs`, `Playing::ready`/`reply`). The runner goes into the shared crates as data: steps in, the messages that may go out now, and a reply offered to what the queue is waiting on releasing the rest. Both clients send what it says and hand it their replies; the host drops its queue for the same runner. **A session's open goes through it too**, so a join waits for the reads of the takes it is made of — which is the empty join above — and a step names which server it goes to, for the one endpoint that has two.

     - ✅ **The runner, and the host on it** *(2026-09-13)*. `clausters_editing::run::Runner`: steps pushed for a `Server` (`Sound`, `Samples`), the messages that may leave with their server, and a reply from a server releasing a wait only if that is the server it was sent to (`Released`, `Refused` with the reason, `Unrelated`). One queue across both servers. The host's `Playing` holds one instead of its queue, every reply reaches it with the leg it came in on (`instance::Leg`, native and browser fronts), and a join minted by a hand is pushed as a stitch waiting in the session and an attach behind it on the player -- `Host::stitching` and its `/done` arm are gone.
     - ✅ **A session's open through it** *(2026-09-13)*. `sources::Load::steps` states the load as steps -- each `/buffer_allocRead` and each `/buffer_stitch` followed by the `/done` of that very buffer, in planned order -- and `clausters-gui --session` drives a runner over the in-process session until it is idle (`drive_session`, bounded at 10 s), instead of sending the batch and counting reads. The session the example writes now opens with every source loaded, the join included, and no `/fail`; and it opens at once, where it used to wait out the whole ten seconds: the count took the stitch for a sixth read, and only the five reads answer as reads.
     - ✅ **The clients on it** *(2026-09-13)*. `clausters_editing::run::call_json` is the runner as one JSON door -- `push`, `ready`, `reply`, `idle` -- bound as `clausters_editing_runner_{new,free,call}` (core ABI **v58**) and `StepRunner` in wasm; `apply::steps_from_json` reads back the steps a playback answered. `Playback._run` and the web client's `run` push a playback's answer, send what is ready, send **the awaited message last** as the request that waits on its reply (`request` in both, matched by command), and hand that reply back. Neither pairs a send with the step after it any more, and neither decides which reply releases what.
  2. **One transport.** The piece's playback is configured differently per endpoint: the clients make it at the root with its graph bound to the transport (`bind_transport: true`), and the host makes it inside a group it governs with the binding off (`bind_transport: false`), a group the take monitor shares. The playback is set up one way everywhere — the crate makes its own group bound to the transport and puts the piece's graph in it — and the host's take monitor stops sharing that group.

     - ✅ **One transport** *(2026-09-13; the monitor's place decided with the user)*. `Op::Transport` makes the transport's group at the top and binds it (`instance::TRANSPORT`), `Op::Graph` names the group it goes in, and the piece's graph is made there; a teardown frees that group with everything inside. `Endpoint` keeps only `chunk`: `target` and `bind_transport` are gone, and so is the three-argument `clausters_editing_playback_new` (core ABI **v59**, breaking), with both clients' `PiecePlayback` constructors. `PiecePlayback::group` answers the group. **The server governs one group**, so a take monitor outside it would stop following the transport: the GUI host's monitor makes a group of its own **inside** the piece's (`Host::monitor_group`), and a host with no piece still binds one of its own, the first time the monitor needs it rather than when the player attaches. The monitor's messages go through the runner, behind the piece's steps, so a reader is never sent before its group.
  3. **One event path.** A client's editor receives a `/gui_event` over its socket; the standalone host's editor is reached by an internal path (`window_verb`, `answer_own`, `answer_piece`) that builds the message itself. The host builds the **same** `/gui_event` a client would receive — the same stamp and version — and hands it to the editor through the same delivery a client's editor goes through, so the only difference left is whether a message crossed a socket or stayed in memory.

     - ✅ **One event, whoever receives it** *(2026-09-13)*. `Host::event_message` builds the `/gui_event` both fronts send -- widget, stamp, the version the host is drawing, payload -- and `Host::deliver` hands that same message to what the host owns; a front sends it on only when nothing took it. `answer_piece` passes the message whole to the editor's `event`, where it used to build one of its own with no version, which applied every edit unchecked. `answer_own` is left as the tests' shorthand for building and delivering one. Held by `an_edit_made_against_a_picture_that_is_gone_is_refused_here_too`. *Found on the way*: the window's undo and redo settled with the tree's version in a piece session (`crates/clausters-gui/PLAN.md`, "Found by use").
     - ✅ **The recorded exchange replayed against the host** *(2026-09-13)*. `the_recorded_exchange_is_answered_the_same_by_the_host` delivers the nine turns of `editor_exchange` to a standalone host through `Host::deliver` and compares what it tells itself, what it asks of the playback, whether the piece changed and the regions it is left with -- widgets, and the `link` prop that names one, by role. Every answer the host gives itself now goes through one `Host::tell` (corrections drawn, stamp settled), which is what a test can read. **Replaying it found three things the host did not do**, each one a client's behaviour it had never had: it did not ask the editor's `settle` after a changed turn, so a name the host minted was never corrected; it answered the window's undo and redo in the tree's own arm, where a client's editor reads them as a step of the history (walked, then `resync_all`, then `acknowledge` -- `Host::step_piece`); and it took the piece's own counter for the conversation's version, which parts from the history's on the first split. The owner keeps that version apart now (`Owner::conversed`): the version a changed turn answered with, and one more per step.

  **What stays different, and why it is not a fourth.** The standalone host's two servers — a session in the process that owns the samples, a player that sounds — are its topology rather than an implementation of something a client does, and the runner in point 1 is what addresses a step to one of them.

  **Acceptance:** `clients/python/examples/editors/edit_multitrack.py` and `clausters-gui --session clients/python/examples/out/edit_multitrack.json` (the session the example writes) sound the same piece by ear — the space bar, the transport row, the ruler's mark, and the join, which plays what it joined; no endpoint keeps a step queue, a transport binding or an event delivery of its own; the recorded exchange of `O32` step 6 still passes, and the host is held by a replay of the same exchange.

- ✅ **O34 - A source is a file, a clipboard holds segments, and a render is temporary** *(opened 2026-10-22 by the user, with the turn it serves: `docs/history/plans/apps.md`, "The turn: an application's takes live on disk"; done 2026-10-22: `clipboard::Content::Segments` of `SegmentPart`s naming files by absolute path and shape, no blob, so it crosses as text; `workspace::Workspace`, temporary or the session's folder; `Session::copy_segments`, `paste_segments`, `add_render`, `release_renders` over the document, the history's held sources and the clipboard, and `for_saving`; no part gained a field, so `FORMAT` stays. The clients carry a clipboard as the string they already pass to the host, so nothing in them changed; the round trip through their editors is `X28`'s, where a copy first makes one)*. The document's model already says most of it: a `Part` names a source and a frame range, and a source is a file or a list of segments. What it lacks is what an editor of files needs to stop holding samples. **The clipboard holds segments**: a `Content` of a list of parts over the sources it was copied from, with the sources' locations, so a copy of an hour is a few parts and a paste into another window or another file is a list edit. **A render's lifetime**: a file the NRT server writes for an edit is a source of the session, temporary until a save keeps it, and deleted when no history entry and no clipboard names it. **The working folder** a session's renders are written to: temporary for the audio editor, the session's own folder for a multitrack (the turn's fifth decision). The wire's `FORMAT` moves if a part gains a field. **Acceptance:** a clipboard of segments round-trips through both clients and the host's clipboard; a copy across two sessions keeps its sources by location; the history's walk frees exactly the renders nothing names, tested against a sequence of edits, undos, a redo dropped by an edit and a save; a saved session names no temporary file.

## What stays out of the crate

**The gesture**, in every form - the drag machine, the draw stroke, the lasso, the marquee, hit-testing, the pending overlay's drawing, the visible refusal when a zoom cannot express an edit. Those are the GUI host's and stay in `crates/clausters-gui/PLAN.md`'s D and H tracks, which are reformulated as the view halves they always were.

**The wire.** The crate defines intents and outcomes; it does not encode them. OSC framing, `/gui_*` addresses and the transports stay where they are.

**Audio processing.** The crate describes edits; it does not perform DSP. The server owns processing, `clausters-core` owns shared numeric work, and the crate owns neither.

**An interpreter.** A generator is code and the crate never evaluates one. See Future directions for the one case where something might have to.

## Definition of done (per milestone)

The project rule: code plus tests, a clear commit message, this file's checkbox, the developer and user documentation where the change touches them, and a commented example when the feature is user-facing - the example being how new visible behavior is checked by eye. A `docs/decisions.md` entry only where a choice has non-obvious context. **And the packages move together**: a change to the intent vocabulary or the document format closes with its pass over the Python client, the web client and the GUI host in the same commit.

## Future directions (to fold into milestones as they firm up)

Every entry carries a checkbox, and one that converges into numbered milestones leaves this list rather than being ticked here. *(Questions that merely have no answer yet are not here - they are under "Open decisions" above, because a decision waiting to be taken and a capability waiting to be built are different kinds of pending and reading them as one is how the first gets settled by accident.)*

An entry that was done without becoming a milestone is ticked and moves whole
to [`docs/history/plans/document.md`](../../docs/history/plans/document.md) ("Future directions: the entries that were done"), so this list
holds what is open.

- ⬜ **A join over overlapping boxes is refused, and what replaces the refusal is open** *(split off 2026-10-07 from the entry above, which is done for boxes that meet)*. The join refuses ("these boxes overlap, and a join cannot state a mix yet"), which is option C's first half. Ways A, B and C, their trade-offs and the leaning towards B are in the entry above; the decision is due before a lane is allowed to hold a join over an overlap, and it has to answer more than two clips overlapping (the clip-fades entry, point 4).

- ⬜ **An interpreter inside a standalone host.** The capability behind the second half of the frozen-generator decision above, which is written to accept it: a host that can re-evaluate a generator (an embedded QuickJS or the like, since a generator is code and code needs an interpreter) becomes a full editor of documents it did not author, rather than one that can only show what was rendered. Two things the decision already fixes and this must not undo - **frozen remains the fallback** for every leaf the interpreter cannot resolve (an unknown kind, a missing script, a language it does not embed), and the interpreter is a **consumer** of the document rather than a part of it, so the crate is unaffected either way. It opens when a real document has to be reopened somewhere Python is not.

- ⬜ **The host keeps the pile and a client reads what changed** *(proposed by the user 2026-09-09, while settling AP5's undo/redo walk)*. Today the pile is the crate's and a client drives it in its own process, so acts four and five of a walk - find the objects holding those structures, hand each its payloads, then redraw - are written once per client. The direction the user named instead: the **host** holds the history, and instead of the client orchestrating callbacks it receives what changed and maps it onto its own objects (an `OscFunc` waiting, and one table from a wire name to a Python object). Two things follow, and both are the argument: the host becomes an editor of a client's objects, and it already holds the data structures a standalone editor needs - which is what `O24`'s three applications over one document are about.

  **It is not hypothetical.** `--session` already does it: `crates/clausters-gui/src/host/document.rs` holds a `History` with two structures registered and walks both in the order the hand made them.

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

  **What it would need, none of it decided:** an application that answers a **subtree** (a panel) rather than a window; ids a parent hands a child, by role or as a range; the host's reports for a subtree routed to the application that owns each widget, and that application's corrections carried back the same way -- the reports already name the widget, which makes this plausible and nothing more; what a parent and a child share (a navigation group, the playhead, the position cursor) when a roll stands inside a multitrack; focus, and who answers a key when applications are nested, which meets `crates/clausters-gui/PLAN.md`'s `G36` (key bindings as configuration) and "The whole interaction vocabulary is provisional…"; and in a client, the events of a subtree carried to the application's core, which is what `MultitrackEditor` does today for a whole window. `Member` is a closed enum, so every application that joins adds its variant.

  *(2026-10-06, the user, over the standalone host's roll opened from the multitrack: an application inside another is **governed by the outer one** -- the multitrack, there -- and so are the keys, the history and the document or documents being edited; and when the outer application's window closes, every window that depends on it closes too.)* Part of the logic this entry has to define. What exists of it: one history for the two already (`Editing`); a host-side dependency between windows (`Host::depend`), which the standalone host's roll uses, so the multitrack's window takes its rolls with it whichever way it closes; and the close is asked only by the outer window, the session's one holder -- a roll is a view of the session and loses nothing by closing (`crates/clausters-apps/PLAN.md`, "The standalone session's windows close without asking"). What does not: the keys a nested application answers being the outer one's, and the rest of the list above.

- ⬜ **Staleness per node rather than per document** *(named while closing O4)*. Today one counter guards everything: an edit is stale if *anything* moved since the picture it was made against, even something in another lane it could not collide with. That is deliberately conservative and it is right for one owner, where the counter moves only for edits the owner itself just applied and a refusal costs one redone gesture. It becomes wrong the moment two hands edit at once, where unrelated work would refuse each other constantly. The refinement is a per-node revision, so an edit is stale only against the node it names - cheap to carry, and it needs the log (O5) to be worth anything, since without an inverse a refused edit is a lost one either way. Opening it early would buy precision nothing can currently observe, which is why it is here and not in a milestone. It shares its seam with the entry below.

- ⬜ **More than one owner of the same document.** Everything here assumes one authoritative owner per resource, which is what makes a version counter enough and what keeps operational transformation and CRDTs out of the design - an order of magnitude of machinery for a problem the system does not have. Two people editing one composition at once would be that problem, and it would be a track of its own rather than a milestone: the point of recording it is that the current design is a deliberate floor, not an oversight, and the version is the seam it would grow from.

- ⬜ **The multitrack has no master in its view** *(the user, 2026-09-24, hearing
  the master's freeze fixed: "There is no master meter; there never was a mixer
  or a master track")*. The master is on the server -- a strip, a meter slot
  (`mixer::METER_SLOT` on the multitrack's graph) and its way out -- and nothing
  above it: no endpoint fills the master's meter slot, the view draws no master
  row or mixer, and the master's gain is the playback's `gain` argument, set by
  nobody a hand reaches. What a master is in the document (a track of its own,
  or the multitrack's own strip), where it is drawn (a row under the tracks, a
  mixer view) and which of its controls are automated is the design; the meter
  is the smallest part of it, since the slot and the rule for its buses are
  already the tracks'.

- ⬜ **The top of a box's fade trapezoid is the region's level** *(proposed
  2026-10-08 by the user)*. The trapezoid's middle segment, between its two
  corners, is a line a hand could raise and lower to set the region's gain,
  and the box would then draw its top at that height. A region has no level
  today -- `Track::level` is the fader, and a region's only gain is its two
  fades -- so this is a field (`Region.level`, a linear gain, unity by
  default and written only when it is not), an intent to set it, the reader's
  multiply, the trapezoid's top made a grab in the curve element (vertical
  only, its two corners moving together) and `Region.level` in both clients.

- ⬜ **A model of instruments** *(named by the user, 2026-11-08, while the
  parts of `O35` were planned)*. The rules that follow from an instrument's
  physics: how many notes it sounds at once, its range, its strings and which
  double stops it can play, between which strings and with what fingering,
  whether it has a sustain pedal. It is a module of its own, not the event's
  and not the part's, and nothing is planned for it. Where it exists, an edit
  could be checked against it and a reading could use it (which staff, which
  string); until then nothing refuses a note for what an instrument cannot
  play.

## Found by use: the running list of fixes

These are not milestones and they are not future directions. They are what
**using the thing** turns up — an eye pass over an example, a path read while
doing something else, a behavior that is correct and unclear — recorded the day
it is found so it is not rediscovered, and kept here rather than inside whichever
milestone happened to be open at the time.

Every entry is a **checkbox**, so what is open reads as open at a glance and
nothing has to be inferred from where it sits. A fixed one is ticked, with the
record of what was wrong and why the fix is the shape it is, and moves whole to
[`docs/history/plans/document.md`](../../docs/history/plans/document.md) ("Found by use: the entries that were fixed"): this list holds what is
open, and that file is where a search for a fixed one ends.

Anything unresolved lives here or under "Future directions", both **after** the
tracks: never inside the milestone that happened to be open, and never among
finished work, where a pending item reads as done.

- ⬜ **A MIDI file and an OSC score are sources, as a sound file is, and a
  session holds neither inside** *(found 2026-09-29, with the user, designing
  `crates/clausters-apps/PLAN.md` `X3.10`)*. `Location::Events` keeps an
  `EventSequence` in the session file, on the argument that a `.mid` would
  lose the ids, the lanes and every key it cannot say (`session.rs`). That
  mixes two levels. A Standard MIDI File is an interchange standard, as WAV
  is, and an OSC score is the server's own offline format; each is a
  **source** the session points at, as it points at samples. What an edit
  needs -- identity, undo -- lives in the document *over* the source, as a
  region and a join live over a sound file that stays as it is.
  - **An event is one kind**: a note for the server, a MIDI message or an OSC
    message, never two at once (its `type` already says which). A sequence
    that mixes MIDI and OSC events is saved as **two files**, a `.mid` and an
    OSC score, rather than as one format that says both.
  - **An `EventSequence` is neither recorded nor a source.** It is the
    client's representation of what a `Timeline`'s instructions render to --
    the events in beats that the clients and the crate edit and play (as a
    lane's data, which the server reads and never edits). What is recorded
    and stored is a file: samples, MIDI, an OSC score.

  **Open:** how the document keeps ids and the edit log over a file that
  carries none (ids minted on load and kept in the session beside the
  reference, or edits as layers over the source); what `to_midi`/`from_midi`
  become once the file is the source; how a session written with
  `Location::Events` opens.
