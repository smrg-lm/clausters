# The engine: threads, buses and the shared segment

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## Threading and the RT-safe boundary

The OSC network thread may allocate, lock and do I/O freely; the audio thread
(the cpal callback, or `render()` in NRT) may do none of those. Everything the
audio thread needs arrives **fully pre-built over lock-free FIFOs**, and freed
memory leaves through a garbage FIFO to be dropped on the network thread.

- **Defs and their control-name tables live only on the network thread.** The
  `HashMap<String, Arc<SynthDef>>` and a mirror `node_id → Arc<SynthDef>` (kept
  from `/synth_new`, cleaned on `Garbage::Freed`) stay off the audio thread, so
  `Cmd::SetControl` is plain-old-data and the audio thread never compares
  strings. `/def_free` only removes the map entry — live synths keep their `Arc`
  (exact scsynth semantics).
- **UGen wiring allocates nothing.** `UGenSynth::process` builds each UGen's
  inputs in a fixed stack array (`MAX_UGEN_INPUTS`) via `split_at_mut` over the
  wires; the topological order guarantees inputs only read earlier wires.
  Guarded by `assert_no_alloc`.
- **Asynchronous command semantics are deliberate.** A `/server_status` immediately
  after a command may report the old count: commands apply at the start of the
  next block. That is scsynth's model, not a race.
- **A queue the audio thread fills is served by a poll, not by a wake.** The
  network loop parks in `recv_from`, and a worker that finishes something ends
  the park with a zero-length datagram (`osc::wake`) — which is why an NRT job
  that took 2 ms is reported in 2 ms. The audio thread cannot do that: sending
  is I/O. So the one queue it produces into — node lifecycle events — buys its
  promptness with the **tick** instead: while any client holds a
  `/server_notify` registration the loop's timeout is `NOTIFY_INTERVAL`, and
  that number is deliberately `MIN_STREAM_PERIOD` rather than a new one. A
  client that asked to be told about the node tree is entitled to the cadence a
  client that asked to be told about a bus already gets, and picking the same
  number keeps "how often may a subscribed client be served" a single answer.
  Nobody listening, nothing to be prompt about: the tick falls back to
  `GC_INTERVAL`, which is housekeeping and was never a latency.

## Buses, execution order, and the network-thread mirror

- **Control buses bypass the command FIFO.** `/bus_set`/`/bus_get` operate directly
  on shared atomics from the network thread; a synth sees the change on its next
  block — the same effect as routing through the FIFO, without the traffic.
- **Execution order is audible and testable.** `Out` sums, `ReplaceOut`
  overwrites; the order tests use a `ReplaceOut(0.0)` "silencer" that wins or
  loses a bus depending on whether it runs after or before the source.
- **The mirror can run ahead of the engine.** The auto-order mirror reflects
  commands *when sent*, so a scheduled future bundle is already mirrored;
  `queryTree` may briefly show that future state and a re-sort converges on the
  next change. Correctness never depends on the mirror (see parallel groups).

## Parallel groups are bit-identical to sequential

The safety of parallelism must not depend on the network mirror (which can run
ahead). So `BusUsage` masks travel *to the engine* inside `Cmd::AddSynth` (and
are re-sent by `Cmd::SetUsage` when a `/node_set` touches a control used as a bus
index), and stage partitioning happens on the audio thread with its own data —
pure bitops, no allocation. A greedy per-block rule in child order closes a
stage on the first bus conflict, so writers to the same bus serialize in order.

Consequence: a stage's members touch pairwise-disjoint buses and never read what
the stage writes, so results are independent of interleaving and the stages
preserve order — **`--workers` changes only wall-clock time**, never the samples.
Golden and RT/NRT-identity tests guard this.

## Denormal protection (three independent pieces)

Subnormals appear in recursive states decaying to zero (filter tails, envelopes,
Faust recursions) and resolve 10–100× slower in microcode on many CPUs — exactly
as a sound fades out. Three guards, kept in lockstep:

1. **`dsp::denormals::flush_to_zero()`** puts the calling thread in
   flush-to-zero mode (MXCSR FTZ+DAZ on x86-64, FPCR.FZ on aarch64, via inline
   asm — the `_mm_setcsr` intrinsics are deprecated). Re-armed in every cpal
   callback **and** armed at the start of `render()`: FTZ changes results, so
   NRT must arm it too to stay sample-identical to live.
2. **`-ftz 2` in every Faust factory** flushes recursive variables below the
   normal range, independent of architecture and thread FPU mode.
3. The one exception: the Faust *compiler* path runs inside
   `dsp::denormals::normal_precision` — libfaust's interval typing aborts under
   FTZ/DAZ, and the guard restores the armed mode on exit.

Guarded by `crates/clausters-server/tests/denormals.rs` and the Faust-tail test in `tests/golden.rs`.

## MIDI: standard channel-voice, byte-identical to OSC, in a shared crate

- **A MIDI voice realizes the *same* OSC commands an OSC client would send.**
  `CmdTranslator::translate_midi` maps note-on → `/synth_new` (with `freq`/`amp`
  from named conversions), note-off → `/node_free` or `/node_set gate 0`,
  aftertouch/CC/bend → `/node_set` on live voices. Reusing the OSC path makes a MIDI
  voice byte-identical to its OSC equivalent, and the reserved voice-ID range
  (`MIDI_NODE_ID_BASE`) stays disjoint from client and `/synth_new -1` IDs.
- **MIDI lives in a reusable native crate, not the Python client.** The original
  plan (MIDI 1.0 in a Python library) was scrapped: MIDI belongs in
  `crates/clausters-midi` (a versioned C ABI, shared by client and server), with
  the message layer in **MIDI 2.0/UMP** for high resolution (16-bit velocity,
  32-bit controllers, no 7-bit loss), persistence to a MIDI 2.0 clip file and to
  `.mid` (SMF) for interop.

## Real-time scheduling: opt-in, then restored as default

A two-step decision worth recording because it reversed:

- **First made opt-in (M24b):** the default `rtprio` promotion tripped RTKit's
  `RLIMIT_RTTIME` watchdog under sustained overload and the kernel killed the
  process with SIGXCPU — a silent death that hung clients. Overload must break
  the audio, never the process.
- **Then restored as default (M24c):** with a SIGXCPU guard in place (sustained
  overload demotes the audio thread to SCHED_OTHER — audio degrades, server
  survives), only the performance cost of *not* scheduling RT remained.
  Measured on one machine: ~300 stable 1-sine nodes as SCHED_OTHER (124% peak at
  36% avg) vs ~500+ as SCHED_RR (92% peak at 34% avg) — the average misleads,
  the callback must fit its **worst** block. An RT-scheduled callback is the
  standard operating mode of every production Linux audio client, so the default
  came back, kept safe by the guard.
- **Then the watchdog's limit was set back where it was assumed to be
  (2026-10-15):** a measurement on a real device found the soft
  `RLIMIT_RTTIME` at *one buffer* — 2.67 ms at 128 frames — not RTKit's
  200 ms. cpal's real-time promotion sets it to the buffer's duration while
  asking RTKit (which requires the process to hold a limit no larger than its
  own), and renews it whenever the quantum changes. At that value the guard
  fired on a single callback that overran its period by a scheduler tick, at
  35% average load, and demoted the thread until restart: one late buffer
  degraded every buffer after it. The callback now raises the soft limit to
  half the hard one after each promotion (`server::rt::lift_rttime`); half,
  because at soft = hard the kernel's SIGKILL and SIGXCPU come due on the
  same tick and the kill is checked first, which would leave the guard no
  turn. The watchdog is then what it was meant to be, a limit on a thread
  that has stopped yielding.

## A bus is a bus: the sample ring is the server's, never an API's

The live data views grew one at a time, and each took whatever number was
nearest the implementation. A `meter` took a control bus, because control buses
sit in the shared segment where anyone can read them. A `scope` took either a
control bus *or* a "tap" — an index into the eight sample rings the segment
carries — because that is the only way an audio bus's samples leave the audio
thread. The goniometer and the spectroscope took only taps, in pairs and runs.

So the surface said there were two kinds of signal, `bus` and `tap`, when the
server has exactly one kind of thing at two rates. Worse, the "tap" number was
not even a bus at a different rate: it is *which of eight canaletas is currently
carrying that bus*, which the caller had to allocate, route (`/bus_tap tapIndex
bus`), thread into the widget, and release. Every layer above the segment —
the wire, the host, both clients, the examples, the books — repeated that
bookkeeping.

**Every view now names `bus`, `rate` and, where it reads several, `channels`
adjacent buses from there.** This is SuperCollider's model (`Stethoscope` takes
an `index`, a `numChannels` and a `rate`, and its `rate` defaults to `\audio`
with a keystroke to flip it), and it makes bus 0 — the first hardware output —
the default a bare `scope()` or `meter()` shows.

The ring does not disappear; it stops being anyone's business but the server's.
`/bus_tap bus watch` replaces `/bus_tap tapIndex bus`: the client asks for a bus, the
**server** picks the ring and publishes the choice in a per-bus directory in the
segment, so every reader resolves bus → samples by lookup. Watches are counted,
so two views of one bus share one ring and the last one to stop frees it. The
GUI host is what turns a widget into that command — it diffs the audio buses its
open documents read whenever a def, a free or a set changes them — and a
`/bus_tapStream` subscription *is* a watch, so a browser client never issues the
command at all.

Why the host and not each client: the alternative was to keep the ring on the
wire and have every client allocate and route before building a tree. That
duplicates the same plumbing in Python, in TypeScript and in wasm, and it breaks
the property that the GuiDef builders are host-agnostic — they would need a live
server handle to construct a widget. Doing it in the host does it once, in the
language the host is already written in, for every client at once.

### The meter needed a data path of its own

Making `meter` an audio-bus view exposed that it could not *be* one. A meter is
the one view in a mixer that exists per channel, and there are eight rings for
the whole system: a stereo master plus four channels would exhaust them, and a
meter never wanted samples anyway — it wants one number per block.

So the segment grew a second per-bus array beside the directory: the **level**,
published for every audio bus every block. Two decisions inside that:

- **The value is a peak held with a decay, not the raw block peak.** A block is
  64 samples; a display frame is a dozen blocks. A reader of the raw value sees
  the last block before it looked and misses the other eleven — which is to say
  it misses exactly the transients a peak meter exists to catch. The engine
  holds `max(block_peak, published * release)` with a 20 dB/s release, the usual
  peak-meter ballistic.
- **A decay rather than a max the reader clears.** The alternative — the engine
  accumulates, the reader `swap`s in a zero — is exact and needs no constant,
  but it is single-reader by construction: the second reader of a bus steals the
  first one's peak. Several readers of one bus is the normal case here (two
  meters in one window, a host and a headless capture), so the value has to be
  readable without consuming it.

The cost is one pass over the block per audio bus per block, one relaxed store —
no allocation, no lock, guarded like the rest by `crates/clausters-server/tests/rt_safety.rs`.

## A render's noise belongs to the render, and its file to the server

Two decisions about the offline path, both taken by asking who owns a thing
rather than what is convenient to reach.

**The seed.** Stochastic UGens drew their per-instance seed from a
process-global counter that nothing reset. A fresh process was reproducible, so
the golden-file tests passed and nobody noticed; but the *second* render in one
process drew different seeds than the first, so the same score gave different
noise. That is a strange property for an offline renderer — a bounce is supposed
to be a function of its score — and the noise module already claimed the
opposite in its own header. The counter now lives in `BuildCtx`, which every
UGen factory already receives, and `CmdTranslator` reserves one contiguous run of
seeds per synth, so the sequence is a function of the score and of
`RenderConfig::seed`. Two synths still never share a stream, which was the
original reason for a counter at all; what changed is only *whose* counter it is.
The seedless constructors went with it: `WhiteNoise::new()` had no correct
meaning once seeding was per-instance, and a default that silently correlates two
generators is worse than no default.

**Where the sequence starts: a random process is unpredictable first.** Moving
the counter into the render left a second question, and the first answer to it
was wrong: the sequence started from a fixed constant, so an unconfigured render
was reproducible and `--seed` gave you a *different* take. That is the testing
answer, not the musical one. When a piece has a random process in it, the point
is that playing it again is another performance; reproducibility is the
exception you ask for, by fixing the seed, and it is worth having precisely
because it is not the default. The client had this right all along —
`RandomContext.seed(None)` draws from `os.urandom` and every pattern is
unpredictable until `main.seed(n)` says otherwise — so the server was the odd one
out, and the asymmetry showed up as soon as it was named.

So a render with no seed draws one (`clausters_core::rng::entropy_seed`), and a
booted server starts its sequence there too. What makes that usable rather than
merely honest is the other half: **whoever draws a seed reports it**.
`RenderStats::seed`, the `--nrt` summary line, `--stats`'s JSON, `stats.seed` in
Python — you play a score, you like the take, and the seed is how you get it
back. An unpredictable default without a reported seed would just be a take you
can never have again.

The cost is that anything comparing two renders for bit-identity must now pin a
seed and say so: the golden scenes, the parallel-vs-sequential test, the
native↔wasm parity fixture. That is an improvement in what those tests *say* —
each now declares that it wants the same take twice, instead of inheriting it.

On `wasm32` there is no entropy this crate can reach (`SystemTime` is not
implemented there), so `entropy_seed` returns the constant and the JS door takes
a seed explicitly: the browser has `crypto.getRandomValues`, and the shell
forwards entropy from the edge that has it rather than inventing any.

**The file.** The Python client used to render into memory and write the WAV
itself, which meant a duplicate WAV writer, a duplicate decision about sample
format, and — because the stdlib `wave` module cannot read a float32 WAV — no way
to read back what it had just written. Meanwhile the server had both halves
already: `render_to_wav` streams a score to disk through hound, and `read_audio`
decodes WAV/FLAC/OGG/MP3/AAC/ALAC/AIFF for `/buffer_allocRead`. Neither was reachable
from a client.

The reading half became an FFI export, because a client genuinely needs the
samples in its own process. The writing half did **not**: the client hands the
score to `clausters --nrt`, the same renderer the CLI drives, and the server
writes the file. That avoids widening the embed ABI for something a subprocess
already does, and it is *faster* — a sixty-second stereo render is 5.7 million
floats, and not marshalling them is the difference between 430 ms and 60 ms. The
cost is that `--nrt` needed a machine-readable mode (`--stats`) so the client
learns the frames, events, peak and RMS without reopening the file, and that the
`clausters` binary must be findable. Both are acceptable for a client that
already requires the server.

What this fixes in the API is the shape of the return. `path` now means **where
the audio goes, not whether there is a result**: every render returns one
`RenderStats`, and `samples` is populated only when the audio did not go to a
file. The alternative — samples in one case, a bare frame count in the other —
makes every caller branch on an argument it passed itself.

**Interleaved stays the currency.** It is the server's own layout (`/buffer_getRange`
indexes `frame * channels + channel`; `/buffer_export` writes the same order), so
audio *going to* the server needs no conversion, and the one place the server
goes planar — Faust's `soundfile` — it converts internally. Deinterleaving is a
client-side convenience for analysis, and it stays in Python rather than the
core precisely because `array` extended slicing is already a C-level strided
copy: crossing the FFI to do it would copy more, not less.

## A ring frame says who wrote it, so one segment carries several clients

Context: everything reaching the in-process/shared-memory engine went through
one ring pair, and the server saw all of it as a single `ClientId::Ring`. But
`/bus_stream` and `/bus_tapStream` are "one subscription per client, replaced on
each call", so two peers over one segment silently took the stream from each
other. The case is not hypothetical and not rare: it is every browser page,
where the script and the GUI host both push through the in-page carrier. The
loss was also **permanent in one direction** — the host only re-subscribes when
its own widget set changes, so once a script replaced the subscription the
meters stayed frozen until a widget was added or removed.

Decision: each ring frame carries a `u32` **peer tag** beside its length —
who authored the packet on the inbound ring, who the reply is for on the
outbound one — and `ClientId::Ring` becomes `ClientId::Ring(u32)`.

- **The tag lives in the frame, not in the segment header.** That is what makes
  this cheap: no field of the header or the data plane moves, so the readers
  that pin offsets by hand (`crates/clausters-gui/src/host/shm.rs`,
  `clients/python/clausters/ipc.py`) need only their version constant bumped —
  exactly the case `shm.rs`'s own comment anticipates. The alternative
  considered, **several rings** (one pair per client), moves the layout, fixes a
  client count at boot, and gives nothing the tag does not.
- **SPSC survives, because the tag is about the packet and not the ring.** One
  producer still writes each ring; an embedder holding several clients funnels
  their sends through it and demultiplexes the replies by tag. The page already
  had that shape — every send crosses into the worklet through one `MessagePort`.
- **The embedder assigns the tags.** There is no handshake and none is needed:
  the server has to tell its clients apart, never name them. A sender that picks
  nothing is peer 0, the single client a segment always had, so every existing
  embedder keeps working unchanged.
- **Replies are addressed, and that removed a door.** A listener now hears its
  own client's replies rather than everything on the wire. Observers — a test
  asserting the host's meters stream, a debug tap — get an explicit `ANY_PEER`
  read door in the web client instead of reaching into another client's
  internals. Two page tests were reading the host's traffic by eavesdropping and
  now say so.
- **The C ABI stays single-peer.** Its one consumer is the Python client, which
  *is* one client; a second tag there would be surface nobody calls.
  `clausters_send_as` can be added additively if a C embedder ever grows a
  second client.

Versioning: the framing changed, so `ABI_VERSION` moves 6 → 7 and, by the
linkage rule, drags the SemVer breaking tier with it. Nothing else about the
segment moved.

## How many buses one subscription may list is configuration, and it is per carrier

Context: `/bus_stream` was born with a constant — at most 128 bus indices,
chosen when the only consumer was one browser canvas' meters over WebSocket and
sized by the sentence "128 pairs fit comfortably in a single frame on every
transport". The command exists for clients that cannot map the shared-memory
segment, so **only a page ever meets it**: a native GUI host reads the buses
straight out of the segment and subscribes nothing. What changed underneath is
what the page became. A browser host subscribes the union over every visible
canvas, so the set grows with the *document*: at a bus per meter, a page of
sixty-four canvases walked into the ceiling by opening widgets, and past it
every further canvas read a bus nobody streamed. The refusal was a `/fail` the
host logged and otherwise ignored, so the page went on drawing stale values.

Decision, in three parts.

- **The ceiling is boot-time configuration, like every other pool the server
  sizes** — `--max-stream-buses` / `[server] max_stream_buses`, default 4096 —
  and the in-page engine carries the same knob, because a page is a deployment
  and not a client: `maxStreamBuses` at boot reaches
  `WebServer::set_max_stream_buses`. A limit only one of the two deployments can
  set is a limit the browser is stuck with, which is exactly the position this
  started from.
- **The effective limit is per carrier, and the server reports it.** A snapshot
  is one message per period and is never split across replies, so a
  subscription its carrier cannot deliver would have every reply dropped —
  silently, on the ring. The request is therefore clamped by what the asking
  client's transport carries in one packet (its frame ceiling on TCP/WebSocket,
  the overview reply budget on UDP and the ring) and the result is appended to
  `/server_query.reply`. One number for the whole server would be a fiction:
  the same server answers a page over its ring and a native client over TCP
  with two different figures.
- **A refusal is not a subscription.** The over-large request is still refused
  whole rather than quietly truncated — a client that believes it subscribed a
  set it did not is the disease, not the cure — and the browser host now acts
  on the `/fail` instead of logging it: it forgets the subscription it thought
  it had (its own belief is what stopped it asking again), takes the ceiling
  the refusal names, and re-subscribes what fits. Before that, it asks
  `/server_query` when its leg attaches and clamps the union to the answer,
  naming in the log how many buses were left out and which knob raises them.

What was **not** done, and why: splitting the union across several
subscriptions. The protocol's rule is one per client, so splitting means
several peer tags or a second connection — carrier-dependent plumbing for
headroom a configurable ceiling gives outright.

## Every buffer is writable, because the alternative promised something the compiler could not keep

A pool buffer used to be immutable: built once, handed to the audio thread through an `Arc`, replaced whole by whichever `/buffer_*` command changed it. That is what made the read path free — a flat `&[f32]` the optimizer can vectorise and hoist out of a loop — and it is why the write-side UGens did not exist. `RecordBuf`, `BufWr` and the whole `BufDelay*` family need one thing the design refused: a buffer two nodes can hold at once, one of them writing.

**The shape first planned was a writable *kind*.** A buffer would declare at allocation whether it could be written, keeping flat storage and today's speed when it could not, and the four per-block readers that take a whole slice (`Osc`, `VOsc`, `Shaper`, `Conv`) would **refuse a writable buffer by name at build**, since a recording is not a wavetable.

**That refusal was not available.** A `bufnum` is a **runtime control**, not a static field — `Osc`'s buffer arrives as an input and can be changed with a `/node_set` between two blocks — so there is no build at which a compiler could see which buffer a reader will read. What a kind could actually have done is fail at instantiation, or read silence, and reading silence from a buffer that has samples in it is a worse answer than reading the samples. The promise was the load-bearing half of the design, and it could not be kept.

**So the kind was dropped and the guarantee moved into the storage.** Samples are `AtomicU32` holding `f32` bits, read with relaxed loads through one door and written through `set_sample` on `&self`. Shape — frames, channels, sample rate — is settled at allocation and never changes, which is the invariant that actually matters: a reader's bounds cannot move under it. Contents can change under anything, at any time, from a UGen or a command. What that guarantees is **per-element atomicity and no ordering between elements**: a reader crossing a writer sees some old samples and some new, never half of one. That is scsynth's own semantics for exactly this case, and it is what a looper crossing its own write head has always sounded like.

**The cost was measured before the choice, not defended after it.** An interpolated random read of 64 frames (`PlayBuf`, `BufRd`) goes 145 → 150 ns, **+5%** — 12 ns per block per reader, about a thousandth of a percent of a 64-frame block's budget at 48 kHz — and **+0%** on the other three shapes that matter (sequential reads, a wavetable hot in cache, the convolver's kernel), where the loads still vectorise. The atomics are what make it **legal to write while another thread reads** — without them a concurrent write is a data race and therefore undefined behaviour, whatever the generated code happens to do — and what they cost is optimizer freedom, not an instruction.

**The consequence is a smaller surface, not a bigger one.** No kind, no flag on `/buffer_alloc`, no branch in any reader, no second storage layout to keep working, and no refusal to explain in the reference. One sentence replaces all of it: a buffer's contents are mutable and only its shape is fixed. The one thing genuinely given up is the flat `data() -> &[f32]` accessor, which is why every reader in the tree had to be visited once — and the golden renders say they came back bit-identical.

## One segment has one owner, and every other server on it is a guest

**Context.** A segment used to belong to one server for its whole life: `--shm
<path>` created the file, truncated it and re-initialised the header. That was
right while the segment was that server's own transport — one process, one
ring. It stopped being right the moment the segment indexed the **samples**,
because the process most likely to be restarted is the one holding the audio
device, and it was also the one wiping what everybody else was editing.

**Decision.** `--shm` opens what is there and creates only what is not, and the
segment carries a **claim**: the pid serving its command plane, in the word that
already existed to keep `transport_clock` aligned. The first server on a segment
takes it, serves the rings and owns the samples — a directory row and a region
per buffer it installs. Any later one attaches to the data plane, maps what the
owner published, publishes nothing of its own, and serves its clients over its
own sockets. A claim whose pid no longer answers is stale and is taken over.

**Why a claim rather than a convention.** The rings are SPSC and there is one
pair. Two servers draining the inbound one would each get half the commands, and
nothing would say so — the failure is silent and intermittent, which is the kind
a rule in a document does not prevent.

**Consequence.** The arrangement an editor wants becomes expressible: the
on-demand session creates the segment and owns the takes, and the player — a
separate process, holding the devices — attaches. Killing the player takes no
samples with it, and the next one adopts what is there. Two things do **not**
follow from it and are stated where they bite: the clocks belong to whoever runs
a device (a session publishes none), and attaching restores the samples and not
the routing, since ports and patches live with the process.

**And the claim is also what collects the dead.** The property above has a cost
nobody paid at first: samples *outliving* its process is the design, so a
segment left by an editor that was killed rather than closed is indistinguishable
from one being kept, and a region is a whole take — a few crashes fill a memory
filesystem with files nothing can tell live from dead. The claim answers that
question too, so **creating a segment sweeps its directory** of segments whose
owner no longer exists. Three things make a sweep that removes a file this
process never created the right shape rather than a dangerous one. Unlinking a
name never invalidates a mapping somebody holds — the same property freeing one
buffer relies on — so the sweep ends a name, not a reader. A claim of *nobody*
is never swept: `0` is a segment created a moment ago as much as one released on
a clean exit, and neither is dead. And the path being opened is never swept, so
recovery keeps its exact meaning — start a server against the same path and it
adopts what is there, which is the case where a dead owner's samples are wanted.
What is *not* offered is recovering it under a different name: an editor names
its segment for its pid, so its next run is a new path, and the previous run's
take is a file the sweep is right to collect.

## The write frontier is the buffer's, and the segment's row is a mirror of it

A take being recorded is the one samples that changes with nothing announcing
it: a `RecordBuf` fills a buffer block by block from the audio thread, which is
the one place that must never send a message. What a writer publishes instead is
a single number — how far it has filled — and everything that draws a recording
reads that number.

It was first published **only into the shared segment's buffer directory**,
which is where a peer that maps the region reads it, and that made it a fact
about *sharing* rather than about the samples. The consequence was invisible
and total: `/buffer_stream` — the command that exists for clients which cannot
map anything — derived its report from that row, so on a server with no segment
it reported nothing, silently, forever. That is an engine inside a page, and a
`clausters` booted without `--shm`: the command's whole audience, missing
exactly where it was needed.

So the counter lives with the samples. `Buffer` keeps its own `written`, every
writing UGen raises it, and a buffer that has a segment sink mirrors the same
number into the row. A stream reads **the higher of the two**, and that is not
belt and braces: a *peer* writing into the shared region raises the row in its
own process, and this server's `Buffer` never hears about it.

The rule generalizes past this field: a fact about the samples belongs to the
samples, and a shared-memory layout is a way to *publish* facts, never the
place they are kept. What a picture needs is the same number whether it maps the
memory, subscribes to a summary, or is the process doing the writing.

## A peer writes samples, announces the span, and asks for every operation

**Context.** With a take mapped, an editor stores a stroke into the very cells
the engine reads: no message, no blob, no reply, no reconciliation. That is the
point of mapping it, and it takes two things away — the boundary that said what
a client may do to samples, and the only signal any other client had that they
changed.

**Decision.** Two rules, both narrow.

*What may be written is **samples**, samples the writer already holds*: a drawn
stroke, a pasted block, a take it loaded. Every **operation** over samples — a
gain, a fade, a reverse, a render, a resynthesis — stays the server's verb and
is asked for over the wire, however easy the mapped memory makes the other
thing. Without that line the mapping quietly re-opens the question S12 refused,
since a client with write access can compute a fade and store the result, and
nothing in the memory would stop it. One place performs audio processing, and it
is the server.

*What was written is **announced**, not carried*: `/buffer_touch bufnum channel
start frames`, which the server broadcasts as `/buffer_touched` to every
`/server_notify` client but the one that wrote. Four integers, and whoever holds
a picture of that take re-reads the span.

**Consequence.** The saving stays the round trip rather than the copy, and the
clients that cannot map anything keep working: a page is exactly who needs the
announcement, because a browser cannot map a file and a message is the only way
it can hear about an edit at all. What a peer may *compute* is unchanged and is
what it always was — what drawing needs: a peak pyramid, an analysis for a
picture.

## The segment's layout lives in the shared core, not in each reader

**Context.** Four processes read the shared-memory segment — the server that
writes it, the GUI host, the Python client, and any later peer — and three of
them mirrored its `#[repr(C)]` layout by hand: the same offsets, the same
constants, a second copy of the buffer directory's seqlock, a second tap-window
reader, a second implementation of the ring's framing. The only thing tying
them together was the ABI counter, checked on attach.

That is not a check. A version number says "we agree about which layout this
is", never "we agree about what that layout is", and the difference showed up
twice in one week. The GUI host was moved to ABI v9 by its *number* and kept a
size check that demanded a segment with no buffer directory, so it refused
every valid segment while compiling perfectly. The Python client declared 1024
control buses against a server that had had 16 384 for months — wrong, unused
and invisible, because a client maps the *file's* length and only a
documentation constant was derived from the number.

**Decision.** `clausters_core::shm` is the definition: the header, the rings,
the directory row, every offset derived from the header, and a `View` carrying
the whole data plane — the clocks, the control-plane claim, the buses and
levels, the tap rings with their tear-free read, the directory with its
seqlock, the ring framing. It is pure atomics over an address, so it compiles
for wasm and is unit-tested without a mapping.

What each process keeps is **getting the memory**: `mmap` of a file, a heap
allocation, Python's `mmap`. That is the genuinely platform-shaped part, and it
is the only part. A non-Rust peer reaches the rest through
`clausters_core_shm_*` (the C ABI): the *shape* — every count and offset in one
call — plus the things that are logic rather than arithmetic.

**Consequence.** `crates/clausters-server/src/server/ipc.rs` lost 700 lines, `host/shm.rs` half of
itself, and `ipc.py` its whole layout section; what is left in each is that
process's own concern. Three `const` assertions tie the constants the engine
also declares (the audio-bus cap, the block size, the pool size) to the ones
the layout is sized for, so a divergence is a build error. And the tests that
used to build a segment by hand from a reader's own idea of the layout — a
mirror checked against itself — build one through the core or through the
server instead.

**What this does not do.** It does not make the layout versionless: `ABI_VERSION`
still gates attaching, because two *builds* can still disagree. What it removes
is the second kind of disagreement, the one a version cannot see.

## The load meter measures roles, and the server times itself

`/server_status` reports the audio thread's per-block load — whether the server
is keeping up. It cannot say *on what* the time goes, and a session that has
grown heavy is asking exactly that: a FaustDef compiling, a soundfile loading
and a burst of commands all cost real time and appear nowhere in that figure.
Outside the process the answer already exists (the threads are named, so
`top -H`, `pidstat -t` and `perf` read it), but that is the platform's answer
and no client can ask for it.

Three choices, none of them forced by the others:

- **The unit is a role, not a thread.** The brackets do not measure threads:
  they measure a block, a stage of a parallel group, a serving turn, an NRT job,
  a compilation. So the wire says `dsp 2`, not a thread name or a tid, and an
  engine in a page — one thread — reports the same rows. The first sketch was
  `/server_threads`/`Server.threads()`, taken from the diagnostic tools, and it
  was wrong twice: a thread is an OS construct, which never names an API
  surface here, and it is not what is being measured.
- **The server times itself rather than asking the platform.** Each bracket is
  a monotonic clock read, which is one vDSO call or its equivalent on all three
  desktop targets (`clock_gettime(CLOCK_MONOTONIC)`, `mach_absolute_time`,
  `QueryPerformanceCounter`) — no allocation, no lock, no kernel trap, so it is
  legal on the audio thread, which is the one place the reading is most wanted.
  Per-thread *CPU* time is the other route and it fails there: on Linux
  `CLOCK_THREAD_CPUTIME_ID` is a real syscall and not in the vDSO, macOS has no
  such clock and answers through `thread_info`, Windows through `GetThreadTimes`
  — three implementations of one number that may not be read where it matters.
  The consequence is stated rather than hidden: `busy` is time the work was **in
  progress**, not per cent of a core, so a DSP worker spinning for its next
  stage is burning a core and reads as idle. That is the right answer to "how
  much of the block budget did this stage take" and the wrong one to "what is
  this process doing to my CPU", and the documentation sends the second question
  to a profiler.
- **The counters are cumulative and the window belongs to the reader.**
  `Counters::take_peak_cpu` resets on read, so two clients polling one server
  steal each other's window — a defect that predates this and that a second
  reset-on-read meter would have doubled. `/server_load` only grows, and each
  client differences two replies into an interval of its own (the `share` field
  both clients compute locally).

The cost, measured with `crates/clausters-server/examples/bench.rs` before and after: under the
measurement floor. The audio thread reuses the stamp the CPU meter already
takes, so a block gains no clock read at all — only two relaxed atomic
read-modify-writes — and a worker pays one bracket per stage rather than per
subtree.

## An event lane is keyed by the position and fed from the network thread

*2026-09-28.* A roll holds concrete data, as a clip of audio does, so it plays
on the transport the way a take does (`PLAN.md`, T8). The notes editor had
played by stamping every event on the transport's **clock** and planning the
whole sequence again from the client on every play, locate and edit — the
transport's own work done a second time outside it, and wrong across a loop,
since the clock does not wrap.

**A lane cannot live where a reader does.** A reader is a UGen, so the audio
thread runs it; a note is a node, and a node is built on the network thread,
because the audio thread never allocates. So the lane's data lives on the
network thread, which keeps the stretch just ahead of the position built on
the engine, and the engine only fires what is already built.

**The engine's lane queue is keyed by position, not by clock.** Feeding the
clock queue, as a client does with `/sched_atTransport`, would need the
position->clock mapping at the moment of a locate, and the network thread
learns where the transport stands only once per block — every jump would land
the plan up to a block off. Keyed by position, the engine does the mapping
itself, exactly as it finds a loop's end: an entry is due when the position
reaches it, so a locate and a wrap need nothing re-stamped, and an entry the
position jumped over waits for it to come back. A note's **release** is the one
thing on the clock: when the start fires, its release goes onto the clock queue
a note's length later, so a note rings its length whatever the position does —
except across a jump, which releases what the lanes sound, as a DAW does.

**Ids.** A lane plays again and again, so a note's node id cannot be in the
data: the server mints one per pass from its auto range, and what comes back
unrun through the garbage FIFO is forgotten there — which also fixed the same
leak for a client's cleared `/sched_atTransport` bundles, whose nodes had
stayed in the mirror. A lane's own id is the client's; a playback that owns one
lane gives it the id of the group it plays into, which is unique among a
client's ids and lives exactly as long as the lane, so no new id space was
added for it.

## Notes are routed like audio: event buses and the `Voices` node (2026-10-15)

*Decided with the user, 2026-10-14.* Notes reached a plugin by three paths --
a live MIDI binding, an event lane and the client's `plugin_event` -- and each
decided on its own what kind of instrument received them. A binding could only
make an instance of its own, never play a node that already existed, and the
client's `Event` built `/synth_new` itself, so a pattern could not play a
plugin. The target was scsynth's simplicity: buses of a few kinds, and every
node reads and writes the buses its controls name.

So notes got a **bus kind of their own**. An event bus is written by whatever
is not a node (the MIDI input, a lane, a client) and read by nodes with a
`notes` control, so a MIDI binding is routing and no longer chooses the
instrument. The events keep their source, which is what lets a stop, a lane
replacement or a free release only its own notes when several writers share
one bus. All buses share one fixed list of events a block, allocated at boot:
a bus is a number, not a buffer, and a quiet bus costs nothing.

**A note never makes a node.** The alternative -- a synth per note on the
event path, as MIDI did -- needs the network thread to allocate and the tree
to change on every note, and it gives a plugin and a def two different
models. A `Voices` node holds its voices from the moment it is built and gives
them to notes on the audio thread, the way a plugin plays its own; what it
needed was for a voice to start over without being rebuilt, so every UGen
gained a `reset` on its own memory and a Faust instance is cleared. Random
streams continue across a reset (a voice's noise is not the previous note's
noise again) and pool delay lines and disk streams are not reset, which is why
a def with `DiskIn`/`DiskOut` cannot be a voice. `/synth_new` per note stays
for whoever wants it, as in scsynth.

Three choices that are not obvious from the code:

- **The binding's half.** An instrument binding still makes its node -- a
  `Voices` node of 16 voices, or a plugin's node -- on a bus of its own from
  the top half of the event buses; a client allocates from the bottom half
  (`clausters_core::ids::binding_event_reserved`). A GraphDef binding keeps a
  voice slot per note, because its notes are its slots, not voices of one
  def.
- **A voice's legato is one held voice.** A `pvoice` line on a node is one
  note joined by the next, by note id, rather than one node continued; a
  sequence rendered from it writes the join on each note (`joins`), so a lane
  plays the same line the pattern did. A def's line in a lane holds the synth
  its first note made.
- **What went.** `/midi_bind`'s gate flag is accepted and ignored (a voice
  closes its gate when the def has one), the clients no longer send it, and a
  program change is the reading node's to interpret, so `/midi_map` refuses
  `progN`.

## A reader with nothing to read sleeps until it is woken (2026-10-27)

The threads that read a take for the audio thread (each transport reader's
own I/O thread in `dsp::stream`, the take reader an application runs beside
its engine in `clausters_app_engine::take`, and the application's own thread
in the host, `host::audio_app`) looked for work every 2 or 5 ms whatever they
had to do. Measured on the audio editor with a take open and nothing playing,
that was 9 % of a core: two readers that a fed stream had made idle for good,
a take reader in front of a full ring, and the application asking its engine
for garbage, two hundred times a second each.

**What wakes a reader is what gives it work**, and the audio thread already
said so for a request and a list put in place (an unpark, a futex wake, which
a real-time thread may make). The one thing it did not say is that a ring
has room, and that is now said **once per fall to half a ring**: after each
block a reader whose ring has fallen to half or less wakes whoever fills it,
and not again until it has been filled past half: over half a second of
lead at the ring's size, where a read takes milliseconds. With every cause of
work waking it, a reader parks for 100 ms as a safety net rather than 2. An
offline render waits for frames the way it did, and wakes the reader before
it waits. The application's thread looks every 5 ms only while something can
land unasked (a render on its thread, a transport rolling or a play waiting
on its cue), a render that ends wakes it, and otherwise it rests 100 ms; an
ask wakes it as it did. At rest the editor's process is now the audio
device's callback and nothing else, 2 to 3 % of a core.
