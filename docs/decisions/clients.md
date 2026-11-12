# The clients and their bindings

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## One seeded random context per script

Per-pattern seeds were a design error (corrected with the user): a piece is only
reproducible end-to-end if everything random shares **one** seedable context —
the sclang model. `main.seed(n)` seeds the root; every `Routine`/`Stream` derives
its own generator at creation from the creating context; a draw uses the running
routine's generator (thread-local), falling back to the root outside a routine.
One root seed reproduces a whole script in creation order, and concurrent
routines stay reproducible *per routine* regardless of wake interleaving. The
generator itself is the core's seeded `Rng` over the FFI, so a seeded script
replays identically in every client language.

## Client defaults for the wheel: sample clock (live only) and an enveloped default synth

Two out-of-the-box defaults, chosen for the common case of a **local** session:

- **The built-in `default` synth carries a gated envelope.** It was a bare
  `Sine(freq) * amp`, which clicked at note-on (level jumps from 0) and at
  note-off (the node is freed mid-cycle). It is now `Sine * EnvGen * amp` with
  a gated ASR — equal-power sine ramps (0.01 s attack, 0.3 s release),
  `doneAction = FREE_SELF` — the same shape as SuperCollider's `\default`. Because
  a click-free note-off *requires* a release ramp, and a direct `/node_free` cuts
  the ramp, the player must release this instrument by **closing its gate**. The
  global event default stays `has_gate = False` (a gate-less custom def is still
  freed directly, so it can never leak); the player special-cases `instrument ==
  "default"` to gate-release. So the safety of the free-by-default choice is kept
  while the built-in sounds clean. *(Its tone was replaced on 2026-10-01 — see
  "The built-in `default` is a subtractive voice with the MPE controls"; the
  envelope and the gate release stand.)*

- **`Session.live()` and `Session.embed()` anchor to the server's sample clock
  by default** (config `[client].clock`, default `"sample"`), rather than
  wall-clock OSC timetags. For a local session the sample clock is strictly
  better — drift-free and sample-exact — and making it the default also
  exercises the `/sched_at` path on every run. It falls back to wall-clock
  gracefully if no master answers, so a client with no Clausters server still
  works; `"monotonic"` opts out for driving a remote or non-Clausters peer.
  `Session.embed()` was excluded at first — the sample-clock tracker reaches the
  server over its own UDP socket, which an in-process server has no endpoint
  for, so auto-locking would only have burned the 2 s lock timeout on a
  guaranteed-failing attempt. It joined once the in-process reader existed:
  the embed handle already publishes the sample counter through shared memory
  (`clausters_clock`), so `EmbedSampleClock` reads it directly — no socket, no
  model, no timeout — behind the same tracker surface `lock_to` drives, and
  `Server.sample_clock()` picks the reader by interface. `render`/`nrt` never
  had a live clock.

## Holes live only in the GuiDef record, so a def is sent once

A bundle mounted twice on one page must not collide: two instances need two
node ids, two buses, two blocks of widget ids. The persisted GuiDef record is
therefore a **template** with two kinds of placeholder — `@symbol` for an id the
caller allocates, `$param` for a value the tag supplies — and the resolver fills
them at mount.

The load-bearing decision is *where placeholders are allowed*: *only* in the
GuiDef record and its `boot` list, never in a def payload. That is what makes a
second instance cheap — the `/def_send synth` and `/def_send graph` payloads are byte-identical
between instances, so they are sent to the server once and shared. It also
forces one authoring rule, which is the right rule anyway:

> A bus, a node or a buffer reaches a def **as a control**, never as a baked
> constant.

`piano_voice` used to do `out_ctl(0.0, env)` — the bus number compiled into the
def — which is exactly why that bundle could not be mounted twice. Written as
`out_ctl(control("env_bus"), env)` it can, and the mount passes each instance
the bus it was allocated. The writer enforces the rule (`check_def_payload`)
rather than documenting it, so the wrong form fails at write time.

Widget ids are deliberately **not** symbols. The template numbers its widgets
locally and the resolver offsets them by an allocated base: twelve widgets would
otherwise mean twelve placeholders for no gain. So `"widgets"` in the manifest
is the *width of the id block* — the highest local id, the root's included —
not a count, because a template may number sparsely (`1`, `10`, `20`) and what
is allocated is a contiguous run wide enough for the numbering. A bundle written
before the contract declares no width at all, and the block is measured from the
template instead, which is what lets yesterday's bundles mount twice.

The resolver is **pure**: the caller allocates between `requirements` and
`resolve`, so nothing about a page's or a host's id spaces leaks into the shared
core, nothing is added to the `/gui_*` protocol, and no state is added to the
host. `validate` is the same pass over the declared defaults, run by the writers
— so an unmountable bundle is unwritable.

## One naming rule beats compatibility with a name nobody types twice

Clausters spoke scsynth's command names because it speaks scsynth's *model*, and
inheriting the vocabulary looked free. It was not. scsynth's rule is
`/<letter>_<verb>`, where the letter is a resource class (`n`, `s`, `g`, `b`,
`c`, `d`, `u`); everything Clausters added had no letter to take, so it grew a
second tier of full-word namespaces — `/graph_new`, `/midi_bind`,
`/transport_play`, `/tap_stream`, `/server_info` — and the protocol carried two
conventions at once. Worse, the seams between them had gone quietly wrong:
`_info` was the reply suffix everywhere (`/n_info`, `/b_info`, `/d_info`) except
in `/server_info`, which was a command; watching a control bus was named after
the resource (`/c_stream`) and watching an audio bus after the mechanism
(`/tap`); `u_` meant a UGen *instance* in `/u_cmd` and the UGen *catalog* in
`/u_query`; and sending a def was `/d_recv`, `/d_faust` or `/d_graph` — two of
them naming a family where the third named an action.

The decision is to **break the command names completely** and spell every one of
them by a single rule, `/<resource>_<action>` with the resource as a full word,
replies as `<command>.reply`, and ranges as `Range`. The reference states the
rule at the top ([`schemas.md`](../schemas.md)), because a convention a reader can
apply is worth more than a table they have to consult.

What made this affordable is that the compatibility being given up was never
real. Clausters had already diverged in the places that decide whether sclang
can drive it — its own def formats instead of `.scsyndef`, a typed `/cmd`
surface, a persistent `/error` toggle — so it was close enough to look like a
drop-in and far enough to fail as one, which is the worst of both. And the model
is what a SuperCollider reader actually carries over: the node tree, the add
actions, the bus and buffer pools, the async barrier. Those are unchanged. Only
the spelling moved, and it moved mechanically (`/s_new` → `/synth_new`,
`/c_getn` → `/bus_getRange`).

Two commands changed shape rather than just spelling, and both for the same
reason — the old name was carrying a datum:

- **`/def_send <family> …`** replaces `/d_recv`/`/d_faust`/`/d_graph`. A def has
  a family whatever command sent it; `/def_query` already reports it, in the
  same three spellings. So the family is an argument, and one command sends a
  def.
- **`/transport_query`** splits off `/transport_set`. One command that queried
  when called with no arguments and set when called with two was two commands
  wearing one name — and only the query has a reply to name.

Pre-1.0, nothing here is frozen; the counterpart obligation is that the four
packages move together, which this change did in one commit.

## `new` is the constructor, and the alternates get names

The Python client's node handles were built with class methods —
`Synth.new("beep", {"freq": 440})`, `Group.new()` — while `__init__` was left
holding the *other* meaning, wrapping an id that already exists:
`Synth(1003, "beep")`. That split was never decided. It is a literal
transliteration of sclang, where `*new` **is** the constructor because the
language has no distinguished initializer, so every constructor is a class
method by that name. Python has `__init__`, and the port kept a shape that only
made sense in the language it came from.

It did not even earn the `__init__` it displaced. Nothing outside the classes
themselves ever built a handle from an id that way: the code that names a node
reported by a responder, a query or the GUI uses the base class,
`Node(id, server)`. So the positional wrapping constructor was serving only the
factories that had taken its place.

So: **building a resource creates it.** `Synth("beep", {"freq": 440},
target=group)` allocates an id and sends `/synth_new`; `Group()` sends
`/group_new`. Naming one that already exists is the alternate, and alternates
get names — `Synth.from_id` / `Group.from_id`, which send nothing. The rest of
the named constructors were always alternates and stay: `Group.graph` (a
different command), `Bus.audio` / `Bus.control` (they pick the rate),
`Buffer.alloc` / `Buffer.read` (they block on `/done`), `Server.boot`,
`FaustDef.from_signals` / `from_source` / `from_box`.

`Bus` and `Buffer` keep a constructor that *names* rather than creates —
`Bus(4, channels=2)`, `Buffer(bufnum)` — and that is not an inconsistency. A bus
and a buffer are slots in a pool of fixed size, so an index is a meaningful
thing to write down: bus 4 is a hardware output on every server. A node id is
not; it comes from the client's `NodeIdAllocator` and means nothing to a reader.
`Bus(4)` says something, `Synth(1003, "beep")` never did.

The web client keeps `Synth.new(server, …)` for now, and the divergence is
deliberate: it has no ambient session, so the server is that method's first
positional argument, and as a constructor it would read `new Synth(server,
"beep", …)` — the server first there, the def name first in Python. The two
signatures converge only once a session resolves the server, so the port is
recorded against the milestone that adds one (`clients/web/PLAN.md`, W18)
rather than shipped as a second, differently-shaped constructor.

## The wasm binding is not held to the C ABI's shape, only to a written decision

`clausters-core` reaches the world through three bindings, and cargo checks only
that each agrees with *core* — never that they agree with each other. The C ABI
is compiler-checked; `clausters/_native.py` restates it by hand in ctypes; the
wasm surface is a third statement. So a function could be added to one and never
reach the others with every build green, which is how a "shared" core stops
being shared.

The two legs are held to different standards, on purpose.

**Python owes the C ABI total coverage**, so its check is a comparison and needs
no list to maintain: `ctypes` caches each symbol on the `CDLL` instance as it is
reached, so after `_configure` runs the instance dictionary *is* the record of
what the binding declared, and the crate's own source is the other side.

**The wasm surface is legitimately partial**, and the first attempt to test it
as "the same set" was the wrong instrument. A browser already has WebSocket;
libverovio is not built for wasm; JavaScript has no `u64`; wasm frees by `Drop`
where C needs an explicit `_free`; the C peak cache is a byte blob its consumer
parses while the wasm one is a live object. Measured, the two surfaces differ in
*both* directions and most differences are correct. A test asserting equality
would have needed some sixty exemptions — and those exemptions are the only
interesting content, which is the signal that the exemption list *was* the
artifact.

So `docs/bindings.md` is a manifest of the shared surface: one row per symbol on
either side, with a verdict when a cell is empty — `idiom` (same capability,
different shape), `n/a` (deliberately absent, with the reason), or `gap`
(missing, nobody has decided). `tests/bindings.rs` fails when a symbol appears
in neither column, and when the manifest names one that no longer exists.

The point is what it does *not* enforce. Divergence stays allowed, because
forbidding it would only teach people to lie to the table; what becomes
impossible is divergence nobody wrote down. `gap` is a first-class verdict for
exactly that reason: the honest answer is often "I have not thought about the
other side", and a manifest that had no way to say so would be filled with
invented rationales instead.

## The ctypes declarations are checked against a header the crate generates

*Decided 2026-11-12, revising the arity-only check of 2026-07.* The first
version of `clients/python/tests/test_native_parity.py` compared the C ABI and
the ctypes declarations by **arity** only, and said why: mapping `*const f32` to
`POINTER(c_float)` by reading the Rust source would state every type a third
time (in Rust, in `_native.py`, in the test's own mapping), which is the drift
the test exists to stop. The arity check and the ABI handshake catch a function
added or a parameter appended; nothing caught a parameter whose width changed,
which ctypes converts without a word and which comes back as a wrong number far
from the call.

What changed is where the types come from. `crates/clausters-ffi/include/clausters_ffi.h`
is **generated** from the crate by cbindgen (a dev-dependency of
`clausters-checks`, never linked into anything shipped), and
`crates/clausters-checks/tests/c_header.rs` fails when the committed file is not
what the crate generates. So the header is not a third statement of the
signatures, it is the first one read back by the toolchain, and the Python test
compares each declared type with it through a fixed table of equivalents
(`uint32_t` is `c_uint32`, `const float *` is `POINTER(c_float)`, any pointer may
cross as `c_void_p`). The table is the only new knowledge, and it is about C and
ctypes, not about any function. The header also compiles on its own, the one
core type that crosses the ABI (the segment's `Shape`) included, which makes it
usable by a C caller as well.

What it still does not check: whether a pointer declared `c_void_p` points at
the right thing (any pointer may cross as an address, as handles do), and, on a
platform where two C types share one ctypes type (`size_t` and `uint64_t` are
both `c_ulong` on 64-bit Linux), the difference between them.

## There is no widget registry to generate the builders from

The GUI layer has the same disease the core's bindings had, one level up: a
widget prop is declared in the host (`crates/clausters-gui/src/host/widget`), in the
Python builder and in the TypeScript one, and **nothing ties the three
together**. The wire is untyped JSON on purpose — that is what lets a widget be
added without touching the protocol — so an unknown prop is ignored and a prop
added on one side and forgotten on another is silent in every build.

The plan for it was to generate both builders from a declarative table taken
from the host's widget registry. Measured, that table does not exist and the
name misleads: `host::registry::Registry` is bookkeeping over
`Map<String, Value>` — it stores whatever props arrive and knows nothing about
which ones a `knob` accepts. The vocabulary lives in the *schema's* two wire
passes, `widget::build` (construction) and `widget::apply` (`/gui_set`), spread
over one arm per kind plus the shared bundles (`Flow`, `EditorProps`, `Range`)
those arms embed. There is a source of truth; there is no table.

Generation was also the wrong goal for a second reason: the explicit keyword
signatures **are** the documentation. The Python builder's docstring is the
widget's user reference (the API page is generated from it) and the TS option
type is what an editor completes. A generated `**props` passthrough would trade
the thing a reader uses for the thing a machine finds convenient.

So the artifact is the one `docs/bindings.md` already established for the core:
`docs/gui-props.md` records every prop that does *not* reach all three surfaces,
with the same three verdicts (`idiom`, `n/a`, `gap`), and
`clients/python/tests/test_gui_props.py` reads the three and fails on a
divergence the table does not name — or on a row that names one no longer there.
The readers differ because the sources do: Python is read by **calling it**
(`inspect.signature`, exact), the TS builders and the host's two passes are read
statically.

Its first act was to surface twenty-eight rows, twenty-six of them pointing the
same way: the host implements the timeline chrome (the playhead, its loop
region, the selection, the vertical window), `docs/gui-protocol.md` documents
it, the TS `TimelineOptions` declares it once for every timeline widget — and
the Python builders name it widget by widget, so `track` and `timeruler` name
almost none of it. The other two point the other way and are the more urgent
kind: the TS `plot` offers `buffer` and `cache` that the host's plot never
reads, so a page passing them gets no error and no effect.

## The house spells it `dump` and `load`, not `dumps` and `loads`

`TempoMap` and `TempoClock` are written out as JSON in every layer, and until
now the Python client was the one that spelled it differently: `dumps` /
`loads` against `clausters_tempomap_dump` / `..._load` in the C ABI, `dump` /
`load` in the wasm export, and `dump` / `load` in the web client. The pair
audit found it, since a Python example and its page twin then made calls of
two different names.

The `s` came from `json.dumps`, and that is the whole of the argument for it —
in the standard library the letter *disambiguates*: `json.dump(obj, fp)` writes
a file and `json.dumps(obj)` returns a string, so the `s` is "string" and it is
there because there are two functions. Here there is one, and nothing to tell
it apart from. Borrowing the letter borrows a distinction the project does not
make, and a reader who knows why `json` has it will look for the file variant.

So the Python client renamed rather than the other three, which is the
direction that leaves one name at every layer a person reads: the C symbol, the
wasm export, `TempoMap.dump()` in TypeScript and `TempoMap.dump()` in Python.
The general rule it settles: **when a house name and a language's stdlib name
disagree, the house name wins unless the language's spelling carries a meaning
here too.**

## The MPE decoder crosses as a handle, because a state block would freeze what it remembers

*2026-09-19.*

`clausters-midi`'s contract is the one every boundary in this project states the
same way: **only flat data crosses** — integers and pointer+length arrays in, a
malloc'd byte buffer out, never a library type. MPE decoding does not fit that
sentence as written, and it is worth saying why before it is made to. A
decoder **remembers**: an RPN parameter number arrives spread over four control
changes (CC 101, 100, 6, 38) and means nothing until the fourth; the zone
layout it declares then governs every later message; and each sounding note
carries its five dimensions until its note-off. So the question this milestone
had to answer first is not what the decoder computes but **how its state
crosses**.

**Two shapes were on the table.** A **caller-held state block** — the caller
allocates `clausters_mpe_state_size()` bytes, resets them, and passes them to
every call — is genuinely possible here, because the state is bounded: sixteen
channels of RPN accumulation, two zones, and at most fifteen member notes each.
Nothing grows, so nothing allocates, and the flat-data sentence holds
literally. The alternative is an **opaque handle**, `new`/`feed`/`poll`/`free`,
with the state private behind it.

**The handle wins, for a reason that is not precedent.** Precedent does point
that way — the crate already does exactly this twice, for the live output port
and the live input port, and the `Input` doc frames the handle-plus-poll pair as
what *keeps* the contract rather than what bends it: no callback ever crosses
the boundary, and the host language keeps control of its threads. But the
deciding argument is what a state block would cost later. **Its layout is the
ABI.** Every change to what the decoder remembers — a sixth dimension, a zone
rule refined, a note table resized, a field widened — becomes a
`MIDI_ABI_VERSION` bump and a rebuild for callers that never look inside the
block and have no use for its contents. The handle keeps that layout on the
Rust side, where it is private and free to change, and the ABI commits only to
the four calls. A decoder is exactly the kind of thing whose internals are
expected to move; freezing them into the wire buys nothing back.

**The shape.** `clausters_mpe_decoder_new` returns the handle;
`clausters_mpe_feed(handle, bytes, len)` takes one raw message;
`clausters_mpe_poll(handle, out, cap, out_len)` dequeues one decoded per-note
message, drained in a loop until it returns 0; `clausters_mpe_free` closes it.
Feeding and polling are **separate calls, not one that returns its events**,
because one message in is not one message out: a master-channel bend touches
every note of the zone, so the output is a queue — the same shape
`clausters_midi_input_poll` already has, for the same reason. Configuring a zone
explicitly (for a controller whose layout is known and which sends no RPN) is a
setter on the handle, which is precisely the thing a state block would have had
to expose as layout.

**It is not behind the `live` feature**, and that is the other half of the
decision. The two existing handles wrap an OS resource and a thread, so they
are gated on `midir`; this one is pure computation. It has to build for wasm,
and it has to be callable from the server as **plain Rust** — `Decoder::feed` /
`Decoder::poll` — with the C ABI as a thin shell over that safe face, exactly as
`Input::open` sits under `clausters_midi_input_open`. Which is also the first
time the root crate depends on `clausters-midi` at all: the two MIDI stacks have
not touched until now, and the shared piece is what joins them.

`MIDI_ABI_VERSION` goes to **3**, following v2's precedent of bumping on an
added surface rather than only on a changed one.

## A played event's curves are values the client sends

An event a client plays -- from a pattern, a routine, a timeline -- carries its
curves (`automation`), and a channel's curve is a playable. A sequence's curves
sound on the server: the lane samples each into a table and a reader beside the
voice follows it, in graphs made per combination of curved controls, which the
lane can make because it reads every event beforehand. A client's event knows
which controls have a curve only when it is played.

The first design kept the server's shape: an event graph per instrument, sent
with its def, a reader added as a slot for each control a note's curves drive,
a private bus per control and a `/graph_map` naming it. Writing the reader
showed what it was. The engine runs in blocks of 64 samples, a curve is
sampled every 64 (`CURVE_STEP`), and a control mapped to a bus reads one value
a block; so a reader that ramps to each value it is set is that value arriving
one block later, and the node, the bus, the map and the sub-group around it
compute nothing a `/node_set` on the voice's own control does not. (The ramp
could not have been built from the catalogue either: `VarLag` is a one-pole.)

So nothing is built. The note is the plain synth it always was, and the client
sets the controls: one emitter per server, which wakes every 50 ms and sends
the instants up to two wakes ahead, one bundle per instant holding every curve
that sounds. What each value is -- a note's curve over its channel's, a bend
adding the two, a channel's curve glided by the lane's own one-pole -- is
`clausters_editing::event_curves::window`, bound by both clients.

Three limits come with it, and each was measured rather than assumed.

- **The engine keeps 1024 timed bundles and drops what does not fit**
  (`SCHED_CAPACITY`). A curve is 750 values a second at 48 kHz, so a whole
  curve sent as it is played fills the queue in 1.4 s; a bundle per value and
  per curve leaves room for about thirteen. One bundle per instant for every
  curve is 75 pending at a 0.1 s lead, however many curves there are.
- **A curve reaches a note from its start to its off.** The lane's reader is
  freed with the voice, so a note's own curve runs through its release there.
  A client is not told when a voice ends, a `/node_set` to a node that is gone
  is a `/fail`, and offline it ends the render; the off is the last instant a
  client knows the node is there.
- **In real time the values are as punctual as the client.** The lane reads
  the transport's position on the server. A client stalled for longer than its
  lead sends its values late, and the engine runs them at once.

The lane is not touched: both paths sample a curve by the same function at the
same step, and differ in where the reading happens.

## A package hands its book to the editor through an entry point

*(2026-10-08.)* The wheel carries the client's book as Markdown in
`clausters/_docs/` — the same pages Read the Docs publishes, the generated API
reference included — so it reads offline, and the place it is read is
clausters-editor's documentation viewer. The editor is an application of its
own that works with any Python, so it may not know where clausters keeps
anything, and it may not import clausters itself: the packages that matter are
the ones the *session's* interpreter imports, which change with the environment.

So the editor asks that interpreter. A package registers its book under the
`clausters_editor.docs` entry-point group, named after the package, as a
callable that returns the folder or `None` (`clausters = "clausters._book:folder"`).
Entry points are the packaging standard's own way for one distribution to offer
something to another it does not depend on; the editor reads the group through
`importlib.metadata` and lists each book under the package's name beside its own
documentation, and its search and its help reach all of them. The answer is kept
until the session starts again, the moment an install or another environment can
change it.

**The book is copied as it is.** Its links are relative `.md` paths and the API
pages anchor their members with `<a id>`, both of which a Markdown viewer that
follows links resolves with no rewriting; the one change is that mdBook's
`SUMMARY.md` is headed by the book's title rather than `# Summary`. A heading's id
keeps `_`, as mdBook and GitHub keep it, so a link written for the published
book lands on the same heading in the editor. And because the API pages are
part of the book, building the wheel needs `pydoc-markdown`:
`CLAUSTERS_SKIP_DOCS` leaves the book out of a development build, and a release
refuses that skip as it refuses the others.

**The table of contents is the book's structure, and the server's book rides
along** *(2026-10-15)*. A list of files titled by their paths does not tell a
reader what each one is, so the editor reads a folder's `SUMMARY.md` as the book
it describes: title, parts, chapters by their titles. That file is mdBook's own
format, not one of this project's. The editor's start page lists every book
with its chapters, drawn from those tables, and a folder without one still reads
as a list of its pages. The first version staged `SUMMARY.md` as an `index.md`
page to read, which threw the structure away; it is now staged as it is.

The wheel also bundles the server, so it carries the server's book too, under a
second entry point (`clausters-server = "clausters._book:server"`). Only the
parts a user reads are staged: the user guide and the library and embedding
chapters, not the developer guide. Whoever reads a book inside an installed
package has already installed it, so the pages and sections about building and
installing are left out of both books (`build_docs.py` names them, and stops
the build when one of them is no longer there). A link to anything left out is
repointed at its published address. The GUI host has no book of its own yet;
its pages stay where they are, in these two.

## A panic stops what an environment plays, and the editor's interrupt key reaches it

*(2026-10-09.)* `free_all` was the client's panic button, and it is not silence:
a bundle already sent — the `latency` ahead of every played event, a
`/sched_at`, a transport's plan — makes a node a moment later, a rolling
transport goes on making them, and a routine on a client clock sends the next
one. sclang's `CmdPeriod` is the field's answer and the model here: one verb that
stops everything and closes nothing.

**`panic` is the environment's**, so the default session and an explicit one
have it alike, and the free-standing `panic()` is the one of the session in
force — the same resolution an ambient `play` uses. In order: the environment's
clocks are emptied (`clear`, not `stop`: they keep their beat and a `play` on
the next line sounds); the timelines on them, or on a transport of its server,
are halted, since one whose clock was emptied would otherwise go on reporting
that it plays; the server is silenced (`Server.panic`: every transport stopped,
the queue dropped, every node freed — and the curves the handle was sending
forgotten, since their notes are gone); and the responders are freed.

**Which clocks are an environment's** needed one thing that did not exist. A
session holds the clocks it adopted, but a clock made with no session ambient
belongs to the default session, which holds no list of them — a hand-made
`TempoClock()`, a root timeline's hidden clock. Every clock is now kept in a
weak registry, and the default session's are those whose session is it or
nobody.

**Two parts reach beyond the environment, on purpose.** The server is silenced
whole, another client's sound included: `free_all` already reached every node
and a bare `/sched_clear` every client's queue, so a panic that stopped only this
handle's transports would be the one partial part. And the responders are the
program's, not a session's — a receiver is shared — so every panic frees every
responder, as `CmdPeriod` does; one that must survive is made `permanent`. The
web client's own responders (the node recycler, the fault reporter, a
timeline's follower, the data streams) are permanent, since a panic must not
break the machinery under the next `play`. The `AppClock` is left alone: what
runs on it draws, and is not played.

**The editor stays a Python editor.** It does not know clausters, so its
interrupt key cannot call `panic` by name. A package registers a function under
the `clausters_editor.interrupt` entry-point group, as it registers its book
under `clausters_editor.docs`, and the driver calls it after the key has
interrupted the session's code — on the main thread, once the evaluation has
stopped, so nothing it ran can start a note after the panic — and only when the
package is already imported, so the key never imports anything. clausters'
hook panics every session not closed yet and the default one.

## A pattern is a function, as a UGen is

*Decided with the user, 2026-10-10.* The patterns were one class each, named
as sclang names them (`Pseq`, `Pbind`), and composed with nothing: sc3 made a
pattern an `AbstractObject`, so `Pseq([74, 60, 46]) + 2` was a pattern, and the
port had left that out. A pattern is now a **lowercase function** —
`pseq([1, 2, 3]) + 60` — built from a generator function under `pattern`, sc3's
decorator, over one generic `Pattern` (and `EventPattern` beside it, which the
decorator chooses). The UGens had already made the same break with sclang's
spelling (`sine`, not `SinOsc`), and the user's reason is the reader's: the
call reads as what it is, a function that describes a generator.

It is possible because **no pattern holds state**: a survey of sc3's 73
pattern classes and both clients' 10 found none that assigns to itself after
it is built. What a walk remembers — a position, a random walk's value, a seed
— is the walk's, in the generator's frame, so a pattern is its function and its
arguments and nothing else. A class per pattern added a constructor and an
`__iter__` around what is a generator, and gave nothing: nothing outside the
tests asked `isinstance` of a concrete pattern class. What an event pattern
adds is not state either but the **incoming value** — the event a `pbind` is
building, which a key's pattern reads — so the generator receives it first,
in both languages (`inval = yield value`, sc3's protocol), rather than through
a convention a later pattern would have to retrofit.

**An event pattern refuses an operator**, where sc3 propagated the event kind
through `Pbinop`: an operator applies to each value, an event has no
arithmetic, and the composition sc3 allowed could only fail later, at the first
value. A function over a whole list (`normalize`) is not an operator either —
a stream may not end — and is applied to the list before it becomes a pattern,
as in sclang, where it belongs to `ArrayedCollection` and not to
`AbstractFunction`.

## A sound file is a client object, and a buffer is never one (2026-10-16)

The clients read and wrote audio files through scattered verbs:
`read_soundfile` decoded a whole file into a render's statistics, a render's
kept samples had no writer at all, and `Buffer.read` took a bare path. A file is
a different structure from a buffer -- one is on the client's disk (or a page's
storage), the other is memory in the server -- so it gets its own object,
`SoundFile`, in both clients: a path (or none, for a file not written yet) and
the shape read from the header. `SoundFile.open` reads the header only,
`read` decodes on demand, `SoundFile.write` writes WAV, `to_buffer` loads it,
and `Buffer.write` answers the `SoundFile` it wrote. `read_soundfile` and
`readSoundfile` are gone rather than kept beside it.

Every byte goes through the server crate's codecs, never a client library: the
header probe sits beside `read_audio` (`nrt::probe_audio`) and reaches Python
through the embed library and a page through the NRT worker's module, and
`SoundFile.write` uses the header and encoder a page already wrote renders
with, so the same samples are the same bytes from both clients. An empty
`SoundFile` is a shape with no path, which `to_buffer` allocates as silence;
the audio editor opens one and asks for a path when it is saved.
