# The protocol and its identities

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## Transport roles: TCP as the default command plane, UDP as the probe

**Decision.** The server accepts TCP on the OSC port **by default** (`--no-tcp`
opts out), and clients connect their command interface over TCP by default while
keeping the UDP *boot-or-attach* probe. Each transport has one role: **UDP** is
discovery and small real-time control (scsynth compatibility included), **TCP**
is the command plane, **shared memory** is the data plane (taps, control buses),
**WebSocket** is the browser's TCP, and the **in-process link** is the packaged
standalone's (no sockets at all).

**Why.** A UDP datagram cannot exceed ~64 KB (the OS rejects the send), and off
loopback anything over one MTU fragments at the IP layer, where a single lost
fragment silently drops the whole packet. That bounds exactly the payloads a
complex application needs to move — whole defs, GuiDef trees, buffer chunks,
query replies — while the deployments this server targets (loopback as the
common case, controlled networks for sound installations) get nothing back from
staying datagram-only. TCP's framing makes size a **configuration**, not a
protocol property: the length-prefix ceiling exists only so an untrusted prefix
cannot drive an allocation, so it is a boot option (`--max-frame`, default
16 MiB, advertised in `/server_query.reply` for clients to size bulk requests
from) rather than a constant — no limit is hard-wired, per the rule that the
project must stay usable as a desktop/mobile application without arbitrary
ceilings. Timing is unaffected by the switch: it rides on bundle timetags and
`/sched_at`, never on arrival time. Replies became transport-aware in the same
move (a `/bus_tapStream` window may fill a whole frame for a stream client; UDP
keeps the datagram-safe clamp), and the IPC command rings deliberately stayed
at 64 KiB — large payloads ride TCP even locally, and growing the ring would
bump the versioned segment layout for no demonstrated need.

## Id allocators are registries of finite resources, never counters

Node ids, buses and buffers are finite server resources fixed at boot; an id
allocator — on either side of the wire — is the **registry** of one such
resource's usage. Three invariants, adopted 2026-07 after an audit found every
allocator violating at least one: every released resource becomes allocatable
again; no monotonically increasing counter anywhere; no operation may lose
track of a resource. The audit's headline: the event player allocated one node
id per note and never returned it, so a long-running session marched the id
space monotonically into the server's reserved ranges (silent duplicate-id
rejections at two million notes) and eventually into `struct.pack` overflow;
the server's own `/synth_new -1` and MIDI-voice counters wrapped `i32` in release
builds; the client bus allocator reused freed runs only at exact width and
would hand out the GraphDef reserved top of the bus space.

The fix is one shared implementation, `clausters_core::registry::Registry` —
a bounded occupancy map with a next-fit hint (releases are reusable and runs
coalesce by construction; a double release or foreign id is refused, not
absorbed; exhaustion is an explicit `None`) — used by the server's reserved
ranges directly and by every client over the core FFI, per the build-strategy
rule that language-agnostic logic lives in the core. Two design points carry
the weight:

- **Release follows the resource, not the request.** A node id returns to its
  registry when the node *dies* — the server releases auto/MIDI ids as `End`
  events drain, a client releases its ids on `/node_end` (never at `/node_free`-send
  time, which could re-hand an id whose node is still alive). The registry is
  passive (events are fed in; it never calls out), which keeps it identical
  across bindings and wasm-compatible. The corollary: an engine rejection
  produces no `/node_end`, so the server broadcasts `/fail` **with the id
  appended** — otherwise the client's in-flight id would be lost, violating
  invariant three.
- **Every capacity is bounded at boot, including node ids**, even though ids
  are "dynamic": concurrent nodes are bounded by the node table anyway, so
  the id space only needs table capacity plus in-flight margin
  (`NodeIdPartition::from_max_nodes` — client 4×, auto 2×, MIDI 2× — replacing
  the magic 2M/3M bases). A bounded registry turns a leak into a visible
  fail-fast error, preallocates once (no growth, no `i32` overflow), and lets
  clients size themselves from `/server_query`'s `max_nodes` by the shared
  formula — by query, not convention. The sanctioned exception is NRT/score:
  no real-time bound and no live `/node_end` stream, so a score client's node-id
  registry is unbounded by design.

**Generous defaults (2026-07-16 follow-up).** With every id space bounded and
recycling, the boot defaults were raised so a limit is only ever hit when
something is genuinely wrong: `--max-nodes` 1024 → **8192** (a node can be a
note; the id partition scales with it automatically), `--max-buffers` 1024 →
**4096**, `--control-buses` 1024 → **16384** (scsynth's own default),
`--max-graph-children` 256 → **512** (per *direct* children of one group — it
pre-reserves 8 bytes each per created group and never multiplies `max_nodes`).
The memory cost is small: ~2 MB of node slab, 64 KB of control buses (the shm
segment's default instance grows to 721 600 bytes), 4 KB per created group.
One capacity now *scales* rather than being constant: the node-event and
garbage FIFOs grow to `2 × max_nodes` at boot (floor 2048/1024), because the
registries recycle off `/node_end` — a dropped end event is a client id that
never returns, so a full-tree mass-free must fit one turnover per drain.

## The verbs divide by state: an element is rendered, a timeline is played

Extending the ambient verbs (`play`, `plot`, `render`) raised the question of
whether an arrangement element should also be playable — `play(song)` reads
naturally, and every other sounding thing goes through `play`.

**Decision:** the verbs follow the generated/generator split the arrangement is
built on. `play` takes what already sounds directly — events, patterns,
routines, defs and bare expressions, and a flat `Timeline` (already generated:
random-access, a `Playhead` just reads it). `render` is the *change of state*:
an `Element` is rendered, never played, and `play(element)`'s `TypeError`
points there. Without a destination, `render` **bounces** — an ephemeral NRT
session plays the source and returns `(samples, frames)`, optionally writing a
float32 WAV — so forward-only sources (an event pattern, a routine, a bare
generator) are renderable too; with a destination it delegates to the
arrangement's own seam (`form.render`), where RT and NRT already differ only by
the destination.

**Consequence:** each verb states a semantic, not a dispatch convenience — you
can read `play`/`render` in a script as "sounds now" vs. "changes state" —
and the historical byte-score `render` survives unchanged as the `bytes`
branch of the promoted verb. The coercions the verbs share live once:
`defs.as_def` (a bare `Ugen`/`Signal`/`Box` into an ephemeral def, used by
`play`, `plot` and `render`) and `render.bounce_def` (a def's offline samples,
drawn by `plot`, delivered by `render`).

## `channels` means something different to each verb, and that is the design

`play`, `plot` and `render` share one expression coercion, which invites making
them share one reading of "how many channels". They must not. `play` and `plot`
are **conveniences** — the interactive front door — and may infer: `plot` sizes
its render from the expression, so a stereo pair shows two lanes unasked.
`render` is part of the **NRT interface**, where `channels` is how many outputs
the offline server *has* — a fact about the machine being configured, not a
property of the graph handed to it.

**Decision:** `render` never derives `channels` from what it renders. What it
does instead is **check**: an expression the coercion laid on more buses than
the render has outputs is writing its surplus onto internal buses that reach no
file, so it raises and names the fix. Only the buses the coercion itself
assigned are checked — an explicit `out(8, sig)` is the caller's own routing and
is left alone.

**Why not derive.** Deriving would make `render` guess the shape of its own
output from its input, which is exactly the coupling an NRT interface should not
have: the same expression rendered for a stereo file and for an 8-channel one is
the *same expression*, and only the render differs. Guessing also hides the real
mistake — asking for four channels of a two-channel render is a mistake worth a
message, not a number to silently adjust.

## Who is asked decides the shape: a catalog, a record, and absence as a state

Introspection had grown one method at a time and ended up with three different
shapes for the same idea. `Server.query_defs`/`query_buffers`/`query_ugens`
returned records; `Server.query_tree` returned a nested dict of ad-hoc keys;
`Server.node_query` — a `<noun>_query` among `query_<noun>`s — returned another
dict; and `Buffer.query` returned neither, filling the *handle* in place (the
web client, whose fields are readonly, returned a filled copy instead: one
call, two semantics). A `BufferInfo` existed but only the plural produced one.

Two questions were being conflated. *What do you hold?* is asked of the
**server**, is about every instance of a type, and answers with a structure of
records. *What are you?* is asked of the **instance**, and answers with one
record — the same one. That maps onto the rule the rest of the API already
follows: a command addressed to a resource is that resource's method. So
`node.info()` and `buffer.info()` replaced the server-side singulars, the
plurals stayed exactly where they were (they are a different question, not a
convenience), and every record became an `Info` dataclass carrying no server
and no commands — data that can be serialized, sent to a view, or kept.

*Kept* is where the resources stop being alike, and the difference is the
server's, not the API's. A buffer's shape changes only when a command of yours
changes it, so the handle keeps its `BufferInfo` and reads `frames`/`channels`/
`sample_rate` off it. A node's controls move on their own — an envelope runs, a
mapped control follows its bus, a `done_action` frees it — so `NodeInfo` is a
photograph and nothing caches it. Same interface, opposite lifetime, stated
rather than implied.

The tree then had no reason to invent a shape: it is the node catalog, so its
entries are `NodeInfo`s and what it adds is the nesting. That needed the wire
to carry as much per tree entry as `/node_query.reply` does, which is why scsynth's
`/group_queryTree` *flag* widened into a **detail level** — 0 and 1 remain what
scsynth sends, 2 appends the maps and inferred bus lists. The client asks for 2
and derives the rest from the structure it already received: a node's parent,
its siblings, a group's head and tail. One reply, no follow-up query per node.
The tree is data first and drawing second, which in Python is exactly the
`repr`/`str` split (the web client's equivalent is the object versus its
`toString()`).

Finally, absence. Three commands had three answers for "it is not there":
`/def_query` reported an empty family, `/buffer_query` zeros (indistinguishable from a
real buffer), `/node_query` a `/fail`. A resource that is gone is a **state of the
resource**, not a protocol error — and a batch query must survive one dead id,
which is why `/def_query` never failed in the first place. So every singular query
now answers with a record whose `exists` is false, marked on the wire by a
sentinel in a field that has no valid negative value: `isGroup = -1`,
`frames = -1`, the empty family. `/fail` is left for what it should always have
meant — a malformed request.

## A group's name is a label, and the id is still the identity

*(2026-08-01, M33)*

A group needed to be callable something. The node tree gives a group everything
a DAW channel has — a handle for many nodes, a place in the order, a lifetime —
except a way of saying which one it is: `1002` is the number the client's
allocator happened to hand out, and a console built out of groups reads as a
list of integers. So groups got names.

The shape of that decision is what matters, and it is the same one in three
places: **the name never replaces the id**.

- **On the wire**, no command takes a path where a node id goes. `/group_new`
  takes the label with the creation — a group is born knowing what it is, which
  is when the client knows too — `/group_name` renames one by id,
  `/group_query` resolves a path *to* an id, and every other command is
  untouched. A label carried by `/group_new` is judged **before** the group
  exists — a refused name refuses the creation, because a half-applied command
  that silently downgrades "a group called `mixer`" to "some group" leaves the
  client's model of the tree wrong in a way only a query would reveal. The alternative — accepting a string wherever a
  target int is accepted — would have rewritten the parsing of some twenty-five
  commands, paid a resolution cost on every message, and given a scheduled
  bundle two defensible answers about *when* the path resolves (at translation,
  or when the bundle fires). Resolving once and commanding by id has none of
  those problems, and it is what a client does anyway: it caches the handle.
- **In a path**, a group answers to its name *and* to its id, and an unnamed
  group contributes its id as the segment. That is what keeps every group
  reachable — naming is opt-in, and a subtree with an unnamed group in the
  middle stays addressable — and it is the reason a name may not be **all
  digits**: a numeric name would speak for another group's id segment. (A
  leading digit is fine; `8bit` is a name.)
- **In the reports**, an unnamed group reports an *empty* name, never its id.
  The id-as-segment is a path-composition rule, not a default name.

**A death has to be able to name what died**, and that is the one place the
"names live in the mirror" rule needed care: the mirror drops a node when its
command is *translated*, while `/node_end` only goes out once the engine
confirms the death. So a departing group's label is kept as an **epitaph** — a
bounded queue in the mirror, claimed by the notification — for exactly that gap.
Bounded rather than exact because a mirror removal does not always produce an
event (the node may already have been gone engine-side), and an unclaimed
epitaph is worth less than an unbounded map.

**Where it lives is the whole performance story.** The name is a field of
`MirrorBody::Group` in the network-side `TreeMirror` and exists nowhere else:
`node::Group` grew no field, `/group_name` queues no `Cmd`, and the engine has
no notion of a name. So the audio thread cannot pay for this feature — not a
byte per group slot, not a branch in `process_block` — and that is a structural
guarantee rather than a measurement. It follows the same reasoning as the
auto-sort analysis: anything a client needs to *say* about the tree belongs to
the mirror, and only what the engine must *do* crosses the FIFO. Paths are
composed on the walk and never stored, which is why renaming a group re-paths
its whole subtree in one command.

**The reply shapes follow the feature, not scsynth.** `/group_queryTree.reply`
was scsynth-compatible field for field, and a group name has to go somewhere: it
goes where a synth's def name goes, after the child count, so every node reads
`id, count, name` uniformly. `/node_query.reply` puts it after `headID, tailID`,
and the node notifications carry it as a last argument for *every* node event,
empty where there is no name. That uniformity was chosen over the two
alternatives on purpose — a separate names-only query would make a client cross
two replies by id to draw one tree, and hiding the name behind a `detail` level
would leave a shape nobody can parse without knowing which level they asked for.

Reading these as a compatibility cost would be a mistake, and the reasoning is
the one already recorded under *One naming rule beats compatibility with a name
nobody types twice*: the compatibility being given up was never real. Nothing
but our own clients speaks this protocol — sclang cannot drive Clausters and the
command names are not scsynth's — so a shape is chosen for consistency, and the
four packages move together in the same commit. A back-compatibility patch for
a reader that does not exist is a wart that outlives the reason for it: the
clients parse the field as always present, because it always is.

## A def is a template, so only an instance can be named

A group carries a label now (the entry above), which raises the next question
on its own: a GraphDef instantiates as a group, so should the def declare what
that group is called? It should not. A def is a **template**; a name is a
property of a **place in the tree**, and the two do not travel together.

Sibling names are unique, and a name that comes from the def repeats by
construction: instantiate the same graph twice under one parent and the second
label is already taken. For the per-voice sub-groups of `/graph_newVoice` it
collides *always* — every voice of one instance is the same def. That leaves
the server three ways out, each worse than an unnamed group. Refusing the
instantiation answers a client that asked for two voices with an error about a
label it never wrote. Creating it anonymous is the silent downgrade the naming
rules exist to prevent. Inventing a suffix (`reverb-2`) keeps the tree navigable
under a name the client cannot predict and has to read back out of it. An
unnamed group is not a hole in the addressing anyway: it answers to its id, and
a path reaches it through its decimal segment.

**Naming the instance is a different feature, and it already composes.** Both
clients allocate the instance's id themselves — no client sends `/graph_new`
with `-1` — so the label is a second message on an id the caller already holds,
`/graph_new` then `/group_name`, in order on the same connection. A `name=`
argument in the clients is the whole of it, with nothing new on the wire.

Carrying the label in `/graph_new` itself would give exactly one thing the
composition cannot: the name riding in the instance's own `/node_start`. That
is recorded here rather than built, because the gap it names is not the graph's
— **a rename notifies nobody**. `/group_name` pushes nothing to the
`/server_notify` clients, so an observer that builds its model from the
notifications misses every rename until it queries. If that ever bites, the fix
is for renaming to notify, which closes the case for every group at once; an
optional string in `/graph_new` would close it for graphs alone, and would add
the protocol's only label sitting in the middle of an argument list instead of
at the end of a fixed group of arguments.

## A buffer write is a copy that replaces the buffer, and it refuses what a read clamps

Context: the `/buffer_*` family could read samples (`/buffer_get`,
`/buffer_getRange`) and never put any back, so a client could show a buffer but
not edit one — the read → edit → write cycle an editor view is made of had no
last step, and the only way to install samples was the embed door
(`buffer_load`), which needs to share the process.

Decision: `/buffer_set` and `/buffer_setRange`, named and shaped as the mirror
image of the two reads (flat interleaved indices, single samples and runs), on
the same NRT queue as the rest of the writing family.

- **Copy-and-swap, not mutation, and it costs nothing to choose.** Buffers are
  immutable and the engine holds a clone of the network side's `Arc`, so every
  job here already builds a replacement and installs it whole — `/buffer_read`
  and `/buffer_zero` do exactly this. A write is one more of those: the samples
  land in a copy that swaps in, the old `Arc` leaves through the garbage FIFO,
  and the audio thread cannot observe a half-written buffer. The alternative —
  writing into the live buffer under a lock — would have been the first lock on
  that path and the first way to tear a buffer mid-block, to save a memcpy on a
  command that is already asynchronous.
- **A write past the end fails; a read past the end clamps.** The asymmetry is
  deliberate and is about what the caller believes afterwards. A short read
  hands back fewer samples than were asked for and says so in the reply's
  `count`, so nothing is lost. A short write would leave the caller believing it
  stored samples the server dropped, which is data loss reported as success.
- **No chunking in the protocol and no change notification.** A multi-megabyte
  edit is several messages the client sizes against the `--max-frame` ceiling
  `/server_query` advertises, symmetrically with how `/buffer_getRange` is
  chunked coming back; adding a second, command-specific chunking mechanism
  would duplicate the one the transport already has. And nothing is told that a
  buffer changed: it would be the family's first push notification, the mirror
  is authoritative, and no reader has asked for one.

**Bulk samples ride as a blob, on both sides, and that is a protocol rule.**
`/buffer_setRange` first shipped taking its samples as float arguments, which is
what the rest of the family looks like — and it was wrong by three orders of
magnitude. Encoding 200k samples as 200k typed arguments took 2.7 s in the
Python client's encoder against 0.1 ms for the same samples as one blob, because
N arguments is N type tags and N encode steps at each end; it is also wider on
the wire (5 bytes per sample against 4). So `/buffer_setRange` carries each run
as one little-endian `f32` blob and `/buffer_getRange.reply` answers with one,
which also removes the declared `count` that could disagree with what arrived.

The protocol already worked this way and the new command was the outlier:
`/bus_tapStream.reply` sends its windows as blobs and `/buffer_export` writes
the same bytes to a file. The rule is therefore written down rather than
re-derived: **a payload whose length scales with the audio is a blob; a payload
whose length scales with the parameters is typed arguments** — which is why
`/buffer_set`'s scattered indices and `/bus_getRange.reply`'s control values
stay as they are. Each client gets one function for the pack and one for the
unpack (`clausters.base.bulk`, `src/base/bulk.ts`), so the endianness check —
a `Float32Array` and an `array('f')` are host-endian, and silently wrong on a
big-endian host — has one owner instead of one per call site.

**The queue is what "the current contents" means, not the mirror.** A job that
rebuilds a buffer from what it holds (`/buffer_read`, `/buffer_setRange`)
snapshots it at *parse* time, from the network-side mirror. That is correct for
one command and wrong for a batch: a chunked write submits every chunk before
any of them completes, so each would copy the same pre-batch contents and the
last one installed would erase the rest — which is exactly what a client's
`set_samples` sends. The NRT queue is the serialization point for buffer
mutation, so it keeps its own view of what it last produced per index and a job
builds on that. It is consulted only while the submitter still has work in
flight for that index, since with nothing in flight the mirror has caught up and
is the authority — which is also what keeps a buffer installed *outside* the
queue (the embed door's `install_buffer`) from being undone by a stale entry.

That in turn is what makes the batch worth sending: the chunks go out together
and close with **one** `/server_sync`, so a long write costs one round trip
instead of one per chunk. Both clients grew a barrier that watches for `/fail`
as well as the sync reply, because a batched send otherwise drops the error
that a per-command `/done` would have raised.

What this does **not** solve is the other half of why a client cannot own its
data paths: over the shared-memory ring every peer is one client, so two of them
still fight over a `/bus_stream` subscription. That is a segment-layout change
and is recorded with the ring's own identity work.

## The port is a parameter, and that is what makes a handle worth attaching to

The server binary bound 57110 and nothing else: `--tcp [port]` and `--ws [port]`
moved their own listeners, but the UDP front — the one a client probes to find
a server at all — was a constant. So one machine ran one server, and the client
had to say so: `Server.boot` refused any handle whose address was not the
launcher's fixed one, because booting could only ever produce a server at 57110
and moving the handle to meet it would hand back a server nobody asked for.

`--port` makes it a parameter. It is the **base**: UDP binds it, TCP follows it,
WebSocket sits ten above, so one number moves the whole server and `--udp`/
`--tcp`/`--ws` only exist to pull a transport off that base deliberately. UDP is
the one that cannot be turned off, being the discovery door. In the client, the
handle's own address is passed through to the process, and the refusal shrinks
to the half that is not a limitation but a fact: booting starts a process *here*,
so a handle aimed at another machine still raises.

- **Ownership is the axis the API turns on**, and it was already implicit in
  `boot`/`close`. Making it explicit gave `attach` its shape: a verb for a server
  this handle did not start, which therefore does not stop it either — `close`
  lets go, `quit` stops it over the wire, `free_all` cuts only its sound. And
  `boot` refuses a port that already answers rather than adopting it, so
  ownership is never acquired by accident.
- **`attach` verifies and reconciles**, which a bare `Server(...)` cannot. A
  handle pointing where nobody answers used to drop every message into a UDP
  void that reports nothing; now that is an error at the verb. And since a
  server this client did not launch may have been booted with other flags, the
  handle re-reads the real capacities (`/server_query`) and sizes its allocators
  from the answer instead of from its own `options`, which for an attached server
  are a guess.
- **The stray server needed a terminal, not an API.** A client that crashes
  leaves a process holding the audio device, quite possibly still sounding, and
  `kill` was the only way out. `clausters stop|panic|status` are client-side
  words on the same console script; every server flag starts with a dash, so a
  leading word is unambiguously ours and everything else still reaches the
  binary untouched.
- **Two servers sharing a def store raced on one temp name.** Both recompile the
  persisted defs at startup and both wrote `<name>.tmp` before renaming, so the
  loser's rename hit ENOENT. The temp name now carries the pid and a counter,
  which is what makes a shared `--data-dir` a feature (a def sent to one server
  is on disk for the next) rather than a hazard. The MIDI port's default name
  follows the port for the same reason: two `clausters` ports are indistinguishable
  to whoever is patching them.

What is *not* solved here is the audio device: two servers open two streams, so
this works wherever the host mixes (PipeWire, CoreAudio) and fails on an
exclusive ALSA device, where the second boot reports it and stops.

## Choosing a carrier is not consenting to the network

Every carrier's flag was a port and nothing else, and the two fronts had drifted
into three different answers about where they listened: the audio server bound
UDP on loopback and TCP and WebSocket on `0.0.0.0`; the GUI host bound UDP and
TCP on loopback and WebSocket on `0.0.0.0`. Nothing said any of it — a `--tcp`
or a `--ws` asked for on behalf of a client on the same machine opened the port
to the LAN, and the one leg that is on by default (the host's TCP) was loopback
only by luck of which file it was written in.

So the flags take the address: `--udp [addr:]port`, `--tcp [[addr:]port]`,
`--ws [[addr:]port]`, on both programs, in one shared parser
(`config::PortChoice::parse`) — a bare port, an address alone (the port still
follows the base), or both. **Loopback is the default in all six legs**, and the
widening is a decision written on the command line: `--ws 0.0.0.0:57120` for
whoever means the network. The WebSocket leg is not the exception it looks
like — a page served from this machine reaches `ws://127.0.0.1` either way, so
the common case pays nothing.

- **The bind belongs to the flag, not to the leg's own module.** `bind_tcp`,
  `bind_ws` and the windowed front's `run` used to name `"127.0.0.1"` or
  `"0.0.0.0"` in their bodies, which is how one front came to differ from the
  other without anybody choosing it. They take a `SocketAddr` now and where it
  points is settled once, where the line is read.
- **A config value says it the same way.** `tcp = "0.0.0.0:57110"` is the same
  string the flag takes, beside the `true`/`false`/port the key already
  accepted, so nothing has two spellings.
- **A typo is an error, not a bare flag.** A token after `--ws` that is not a
  bind used to fall through as "no argument" and resurface as `unknown
  argument: 0.0.0.0:57121`; it is now reported against the flag that owns it.
  Addresses are literals — a hostname would resolve to several and the flag
  binds one.

**Remote is then something asked for**, which is where it belongs, and two
things degrade under it — neither of them a bind, both worth naming before they
are found by ear. A GUI source's `path` carrier writes a temp file the host
maps, so client and host are assumed to share a filesystem; over a distance only
the inline ceiling is left. And `--data-dir` names the *host's* GuiDef store,
not the script's.

## A reply opens on its first real field, so `/server_status` lost its leading pad

scsynth's `/status.reply` opens with an unused `1`, and this protocol's
`/server_status.reply` copied it. Nothing here ever read it: the field is a
scsynth artifact that arrived by inheritance, and both clients skipped past it
before parsing anything.

It was dropped, and the reply now starts at `ugens`. What made that worth a
breaking change to a documented wire rather than a curiosity to leave alone is
that it was the **only** reply in the protocol shaped that way. `/server_query`
opens on `audio_buses`, `/node_query` on `nodeID`, `/buffer_query` on its
bufnum; every one of them is read positionally by clients that count from the
first real field. `/server_status` made its readers count from the second, and
that off-by-one is exactly the kind of thing a client gets wrong once and then
carries — the more so now that both clients parse the reply into a
`ServerStatus` record, where a padding argument names nothing at all and only
shifts every index beside it.

The compatibility this gives up is with **nobody**: the command names already
diverge from scsynth (`/server_status`, not `/status`), so no sclang client
speaks this protocol, and a padding int is not what would let one. The trailing
`late_blocks` stays appended, which is the extension point that does earn its
shape — a reader that stops at the eighth field keeps working.
