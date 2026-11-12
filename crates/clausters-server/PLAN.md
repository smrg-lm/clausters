# Implementation plan: a real-time scsynth-style audio server

A synthesis server in Rust controlled over OSC, inspired by the architecture of
scsynth (SuperCollider): a process that opens the audio device, keeps a node tree
(synths and groups), and receives OSC commands over UDP to create/destroy synths,
set parameters, manage buses and buffers, all with sample-accurate scheduling.

## Design principles (non-negotiable)

1. **The audio thread never blocks**: no `malloc`/`free`, no locks, no syscalls,
   no I/O inside the audio callback. See the `realtime-audio` skill.
2. **All communication with the audio thread is lock-free**: SPSC ring buffers for
   incoming commands and for returning "garbage" (memory to free) to the non-RT thread.
3. **Block processing**: blocks of 64 samples (like scsynth), not sample-by-sample,
   to amortize UGen dispatch.
4. **Conceptual, not binary, scsynth compatibility**: same model (node tree,
   buses, buffers, SynthDefs, commands `/synth_new`, `/node_set`, etc.) but our own
   SynthDef format (not the binary `.scsyndef` format — at least not in v1).

## Thread architecture

```
┌─────────────┐  OSC/UDP   ┌───────────────┐  SPSC cmd FIFO  ┌──────────────┐
│ OSC client  │ ─────────> │ Network thread│ ──────────────> │ Audio thread │
│             │ <───────── │ (parse OSC,   │ <────────────── │ (cpal        │
└─────────────┘  replies   │  alloc        │  SPSC garbage/  │  callback,   │
                           │  pre-built)   │  reply FIFO     │  DSP)        │
                           └──────┬────────┘                 └──────────────┘
                                  │ slow tasks (disk, decode)
                           ┌──────▼───────┐
                           │ NRT thread   │  (loading files into buffers, etc.)
                           └──────────────┘
```

- **Network thread**: UDP socket, parses OSC (`rosc`), builds commands *already
  fully allocated* (e.g. the Synth node already instantiated) and pushes them to the FIFO.
  The audio thread just "plugs them in" — O(1), no allocation.
- **Audio thread**: cpal callback. Each block: (1) drains the command FIFO,
  (2) runs scheduled bundles whose timestamp falls in this block, (3) walks the
  node tree in order and runs the DSP, (4) pushes dead memory to the garbage FIFO.
- **NRT thread**: reading/writing audio files for buffers (`/buffer_read`,
  `/buffer_write`), like scsynth's "NRT thread".

## Crates

| Crate | Use |
|---|---|
| `cpal` | Cross-platform audio I/O (ALSA/JACK on Linux) |
| `rosc` | OSC 1.0 encode/decode (messages and bundles with timetag) |
| `rtrb` | Lock-free, realtime-safe SPSC ring buffer |
| `basedrop` | Shared pointers with deferred deallocation off the RT thread |
| `hound` | WAV read/write for buffers |
| `assert_no_alloc` | In tests/debug: panic if the audio thread allocates |

## Core data structures

- **`NodeTree`**: a tree of `Group` and `Synth` with integer IDs (ID→node map,
  pre-allocated or slab). Execution order = depth-first traversal, like scsynth.
- **`Synth`**: an instance of a `SynthDef`: a vector of built UGens + wiring buffers
  ("wires") + control values.
- **`SynthDef`**: a topologically-ordered UGen graph, with constants, named controls
  and wire assignment. Defined in our own format (see M3).
- **`Bus`**: global arrays of audio buses (per block) and control buses (one value);
  the first N audio buses map to the hardware outputs/inputs.
- **`Buffer`**: a pre-allocated pool of sample buffers with channel/frames/samplerate,
  filled by the NRT thread.
- **UGen**: a trait with `fn process(&mut self, ctx: &ProcessCtx, inputs, outputs)`
  over blocks of 64 samples; dynamic dispatch (`Box<dyn UGen>`) is fine in v1
  (construction happens off the RT thread; the per-block virtual call is cheap).

## OSC protocol (a subset of scsynth)

The full, current command set is the canonical wire reference in
`docs/schemas.md` (the single source of truth; the `scsynth-osc` skill is the
working map). It is no longer restated here — this plan tracks *what to build
next*, not the shipped protocol.

## Initial UGens

Oscillators: `Sine`, `Saw` (PolyBLEP), `Pulse`, `WhiteNoise`, `Phasor`.
Filters: `LPF`/`HPF` (biquad), `OnePole`, `Lag`.
Envelopes/control: `EnvGen` (with done actions: free self, like scsynth), `Line`.
I/O: `Out`, `In`, `ReplaceOut`. Buffers: `PlayBuf`, `BufRd`. Math: binary/unary
operators between signals. See the `ugen-dsp` skill for the algorithms.

## Closed tracks

The tracks whose every milestone shipped are in [`docs/history/plans/server.md`](../../docs/history/plans/server.md),
moved there whole; the plan keeps what is open. The ticked entries of "Found by
use" and "Future directions" are at the end of the same file. The tracks are,
by their headings:

- Milestones
- F fork — SynthDefs via Faust (Box/Signal API + JIT)
- Future milestones (M9+) — additional features
- B track — the engine in the browser (wasm)
- U track — the UGen library
- J track — third-party plugins: a `plugin` def family, out of process
- R track — The second review: the shape of the workspace (2026-11-11)

## T track — The governing transport (a pause that stops time)

Section added 2026-08-02. M22's transport is a shared beat grid plus an advisory
rolling state the server never schedules audio from. T makes it able to *stop
time*: a group bound with `/transport_group` is frozen with its internal state
intact, the transport clock stops, and the queue of anything scheduled against
that clock stops falling due — all at the same sample. Part of the server, not a
Cargo feature: every build has both clocks and both queues.

- ✅ **T1 — The governing transport** *(done 2026-08-02)* — two clocks
  (`transport_now = now - frozen_total`, made two Rust types so an axis cannot be
  swapped silently), two scheduler queues with bundles routed by where their
  messages point, `/transport_group` and `/sched_atTransport`, the transport
  clock in the shm header (ABI v6, in reserved space so no offset moved), a
  `TempoClock` that freezes and thaws, `Transport.resume()` distinct from
  `play()` (MIDI's continue against start), and `form`'s `locatable` — a
  resident generator has no position, so a locate over one refuses and names
  the way out rather than faking it. Proven by cutting the frozen span out of a
  paused render and asserting it equals the unpaused one sample for sample, over
  a seeded-noise def, with a deliberately non-block-aligned pause.

- ✅ **T5 — The transport has a position, and the engine owns it** *(done 2026-08-16; opened the same day, out of the GUI's session mode wanting a playhead and a seek — `crates/clausters-gui/PLAN.md`, H5, which consumes this and is written against it. Opened here rather than there on the user's rule: **the server is the only thing that manages playback time**, so anything that computes where a piece is belongs in the server even when a window is what asked for it.)*

  **What is actually missing, and it is not what the name suggests.** T1 gave the transport a rolling state the engine enforces, and `/transport_locate` looks like the seek an editor wants. It is not, and the reason is one fact: **the position never reaches the engine.** `Transport { origin_sample, tempo, playing, position, group }` lives on the network thread (`crates/clausters-server/src/osc/server/mod.rs`), and exactly two commands cross to the audio side — `Cmd::TransportRun { rolling }` and `Cmd::TransportGroup { id }`. A locate sends **nothing**; it changes a number and broadcasts it. So today the server owns *whether* the piece is rolling and not *where* it is, and nothing in a graph can read the piece's time.

  The published clock does not fill the gap either, and the reference currently claims it does. `transportSample` is samples **elapsed** under the transport (the device clock minus the total time spent stopped) — monotonic by construction, so **a locate does not move it**, while `docs/schemas.md` calls it "the time of the *piece*". Elapsed and position are two quantities and only one of them is the piece's time.

  **What follows from that, and is the whole reason this is a milestone**: with no position on the engine's side, every reader invents its own — a `PlayBuf` free-running from frame 0 — and anything an editor wants on top (start here, loop that span, show me where it is) has to be assembled by the client out of arithmetic on a clock. That is a client managing playback time, and it is exactly what the rule above forbids. It also does not scale to the thing this is all for: a multitrack is many readers and **one** time.

  **The shape: a reader follows the transport.** Chosen 2026-08-16 over the alternative of giving each reader its own seek (`PlayBuf` growing scsynth's `trigger`/`startPos`/`doneAction` — a real gap, recorded as **S17**, and deliberately *not* what this rests on). Four pieces:

  - **A position on the engine, in samples**, advancing per sample while rolling, holding while stopped, and **jumping on a locate** — which is the first time `/transport_locate` sends the audio thread anything at all. It is a *second* quantity beside the elapsed count, never a redefinition of it: the transport scheduling queue is on the elapsed axis and needs it monotonic (a jumping axis makes "due" ambiguous), so `/sched_atTransport` is untouched — pinned by a test, since that is the half of "nothing else moved" that code can check. What does change is the reference: `transportSample` is documented as elapsed, and the sentence calling it the piece's time now belongs to the position.
  - **Published in the segment**, because that is how a host reads a clock with no messages. It **fits in the header's reserved space** (`_reserved: [u32; 2]` at offset 56, 8-byte aligned — the same room ABI v6 put the transport clock in), so no offset moves and the out-of-process readers that pin offsets by hand take a number change and nothing else: `crates/clausters-gui/src/host/shm.rs`, `clients/python/clausters/ipc.py`, and the web reader. `ABI_VERSION` 7 → 8, with what that drags for the package version read off the `release-versioning` skill rather than decided here. **After this the header has no reserved space left** — the next counter costs a real layout change, and that is worth knowing before it is spent.
  - **Readable from the graph**, so a reader can follow it: a UGen reporting the transport position in frames, per sample at `ar` (a reader needs consecutive frames within a block) and block-constant at `kr`. A governed node is frozen while stopped and does not process at all, so it holds with everything else and reads the new position on the next play.
  - **Loop points on the transport** (`/transport_loop`), by the same rule that puts the position here: a loop that wrapped by having a client locate on each pass would be a client managing playback time once a bar. With the position wrapping in the engine, a loop is seamless and costs the graph nothing.

  **What this dissolves, worth recording because it was a whole discussion.** With the reader following the transport there is no "one shot" to arrange: the transport rolls past the end of the material, the reader clamps or goes silent, and the head keeps moving because time keeps moving — which is what a DAW does. No self-freeing node, no timer, and nothing computing a duration.

  **What it does not take, named so it is not read as included.** A locate does **not** re-arm the transport scheduling queue: jumping backwards over `/sched_atTransport` entries does not make them fall due again, and jumping forwards does not skip them, because that queue is on the elapsed axis by the decision above. Re-arming a queue on a jumping axis is genuine DAW behaviour and a milestone of its own. **T2 is likely dragged in**: a position in samples crosses exactly the beats↔samples conversion whose origin T2 says is still read on the device axis, so where those two meet is settled here or T2 is closed first — decided when this is started, not now.

  **What shipped** *(2026-08-16)*. `PiecePosition` and `PositionAnchor` in `server::clock_axis`, beside the two clocks and typed for the same reason; `Cmd::TransportLocate` and `Cmd::TransportLoop` reaching the audio thread, the first applied at the sample it lands on (the engine grew a `cursor` for that, the precision `frozen_total` already had); the block cut at a loop's wrap, with a wrap on the boundary belonging to *that* block because the position published at a block's end is what the next block's first sample plays; `TransportCtx` on `ProcessCtx`; the `TransportPos` UGen with its `offset`; `/transport_locateSample` and `/transport_loop` on the wire with three appended reply fields; ABI 7 → 8 with the position in the last of the reserved header space and both out-of-process readers following; the builders in Python and TypeScript, and `transport_pos` in the Python UGen vocabulary. The rationale is in `docs/decisions.md`, and `crates/clausters-server/examples/transport_seek.py` is the manual test — four one-second tones, so a seek, a pause and a loop are each audible the moment they happen.

  **What it did not need, which is the finding.** T2 was named as likely dragged in and was not: the beats→samples conversion deliberately ignores `originSample`, because that origin anchors the beat grid on the *device* axis for phase-aligning clients while the **piece's own axis starts at its own 0**. Saying that out loud settled the conversion without touching the grid, so T2 stays exactly as open as it was and about exactly what it was about.

  **What it left open, deliberately.** A transport addressed in samples still needs a beat grid defined before it will roll, because `/transport_play` has always required one — which for an audio editor means calling `/transport_set` with a tempo it never reads. Relaxing that means the rolling state existing independently of the grid (`defined` becoming a flag rather than an `Option`), which is a change to a command that already works and did not belong inside this one. Recorded in "Found by use".

  **Acceptance:** a locate while stopped moves the published position, and the graph reads the new one on play; a reader following the position renders the same samples as the same span expressed in a score and rendered in batch (T1's own technique, over a seeded def, deliberately not block-aligned); loop points wrap with no client in the loop and no discontinuity at the seam; the elapsed clock stays monotonic across a locate and `/sched_atTransport` behaves exactly as before; `crates/clausters-server/tests/rt_safety.rs` stays green. **The packages move together**: `/transport_query.reply` gains the position **appended** (so a client reading only the older fields keeps working), `/transport_loop` and whatever the locate grows get their builders in both clients, and `docs/schemas.md` plus `docs/sample-clock.md` carry the two-quantity distinction — which is the part most likely to be misread, since one of them is called "the transport clock" and is not the piece's time.

- ✅ **T7 — The transport stops at an end mark** *(done 2026-09-24; asked for by the user
  2026-09-24, out of the audio editor: "on reaching the end of the file, playback has to stop and the play cursor goes back to where the position cursor is"; taken before T6, which then carries it per
  transport)*. The transport rolls until something stops it, so a take played
  without a loop plays on past its end forever, and a multitrack past its last
  clip. What says where a pass ends has to be the engine's, for T5's reason:
  the server is the only thing that manages playback time, and a client
  watching the position to send a stop would land late by its own latency.

  **The shape.** `/transport_end [<int64 end> [<int64 return>]]` sets the
  **end mark**, in samples, and with no arguments clears it. When the rolling
  position reaches `end` the engine stops there, **on that exact sample** —
  what a `/transport_stop` landing on it would do: the governed group and the
  transport clock freeze — and locates to `return`, or leaves the position at
  `end` when none was given. The change is pushed to every `/server_notify`
  client as a `/transport_query.reply`, like any other, so whoever holds
  readers learns the pass is over. The mark stays set, so the next play from
  `return` ends at the same place. The transport never rolls past its mark: a
  play from at or past it stops at once. **A loop wins**: while one is set the
  position wraps and never reaches the mark.

  **Shipped as that shape.** The reply grows the two fields, appended
  (`endSample`, `returnSample`, `-1` for none), so a client reads the mark back
  as it reads the loop. The engine tells the network thread through the
  garbage FIFO (`Garbage::TransportEnded`), the one it drains on every turn,
  idle ticks included; the builders are `transport_end` / `transportEnd`, with
  the return spelled `back` because `return` is a keyword in both languages.

  **Still open:** how the stop meets X7's stopping phase, whose declick ramp
  has to start *before* the end to finish on it.

  **Acceptance:** an NRT render stops at the mark's exact sample, not
  block-aligned, and the next block reads the position at `return`; a loop set
  over the mark wraps and never stops; a mark behind the position stops a play
  at once; `crates/clausters-server/tests/rt_safety.rs` stays green. **The packages move together**:
  `docs/schemas.md`, `docs/sample-clock.md` and both clients' builders in the
  same pass.

- ✅ **T6 — Several transports on one server** *(done 2026-09-24; made a
  milestone and taken first by the user, 2026-09-23; it was a future direction
  since 2026-09-17, left for when there was a case that needed it)*. The case: the audio editor gets nodes of
  its own on the server (`crates/clausters-apps/PLAN.md`, `X7`), with its own
  play, pause and locate, and a multitrack on the same server has its own.
  `/transport_group` binds one group and the server has one transport -- one
  governed group, one transport clock (the frozen total), one position anchor,
  one loop, one transport-axis queue -- so the two would share one playhead.

  **Several transports, not several groups under one.** Each transport is
  independent -- its own play, pause, locate, position and loop -- as the
  future direction already had it; what T6 does not do is bind more groups to
  a single transport, which would give them one shared playhead (the user,
  2026-09-23).

  **The shape** (from the future direction it replaces): a transport becomes a
  resource **addressed by id and sized at boot** (like `--taps`), so the audio
  thread never allocates. Each has its own governed group, clock, position
  anchor, loop and transport-axis queue. The id reaches every surface that
  names the transport: the `/transport_*` commands and `/sched_atTransport`,
  the `TransportPos` UGen, the per-transport clocks and positions in the
  shared-memory segment, the change notifications, and `/gui_headClock`, which
  says which transport a view draws from. `ProcessCtx::transport`, which every
  UGen reads today, becomes the transport the node's governed group belongs
  to -- so a node reads the state of *its* transport, and a node governed by
  none reads a stopped one, as it does today with no group bound.

  **Compatibility:** transport `0` is what exists today, and a command that
  names no transport means it, so a client that never names one keeps working.
  `timeline.transport = server` was named to survive this (2026-09-17): a
  server with several transports makes it hold one of them
  (`server.transport(id)`), and nothing else in the surface moves.

  **Open:** how many transports a boot sizes by default, and whether the
  `/transport_*` commands take the id as a first argument or as a new address
  family. What stays outside a governed group -- the multitrack's master, the
  audio editor's output (`X7`, and the document plan's "The master freezes
  with the transport") -- sits under a parent group beside it, and is not a
  question for T6.

  **Decided and shipped** *(2026-09-24)*. **The id is the first argument of
  every transport command, always** (the user, 2026-09-24): a leading
  *optional* id does not parse, because the server takes an `int32` where it
  wants a sample and `/transport_end 5` would be a mark or transport 5's mark
  cleared -- so the compatibility clause above, a command naming no transport
  meaning 0, was dropped, and every client was moved in the same pass. The
  reply grows the id, appended; `/sched_atTransport` and `/sched_clear
  "transport"` take it too, and `/server_query.reply` appends the count.
  **8 by default**, at most 64 (`--transports`, `[server] transports`). The
  engine holds a table of `TransportState` and cuts the block by the union of
  every transport's queue and edges; binding a group tags its slot, and the
  walk hands a tagged subtree its own `TransportCtx` -- a node reads the
  transport governing its nearest governed group, and transport 0 under none,
  so no def names a transport. A group is governed by one transport at a
  time. The segment's two counters became a 64-row transport table after the
  header (ABI 12, core ABI 69). The clients: a `Server`'s methods address
  transport 0, and `server.transport_at(n)` / `transportAt(n)` is the server
  through transport `n`, which a `Timeline` takes as its `transport`
  unchanged -- the shape `timeline.transport = server` was named for, spelled
  `transport_at` because `transport()` is already the grid's query. The host's
  `/gui_headClock "transport"` takes an optional id after the word, which
  parses because the word comes first. The multitrack and the host's monitor
  stay on transport 0. `docs/decisions.md` has the reasoning;
  `crates/clausters-server/examples/transports.py` is the manual test, two takes one per side, each on
  its own transport.

  **What it left open, closed the same day.** The head clock was one per host
  (`crates/clausters-gui/PLAN.md`, Found by use), and a node outside every governed
  group read transport 0, which is not what `X7`'s output wants. Both were
  decided with the user: **`/gui_headClock <id> <which> [transport]`**, the id
  a window or a widget and never optional, resolved per view -- a widget draws
  from the counter named on it or its nearest ancestor, then its window's,
  then the host's `--clock` -- so a window can hold several views playing
  several timelines; and **`/transport_follow <t> <group>`**, a group whose
  nodes read transport `t` without being frozen by it or routed to its queue,
  which is where `X7`'s output goes. The reply appends the following group.
  `MultitrackPlayback::transport` answers which transport a multitrack plays
  on, so a client names it rather than repeating the number.

  **Acceptance:** two transports on one server play, pause, locate and loop
  independently, each frozen and thawed at its own sample; a node reads its
  own transport's position; transport `0` behaves exactly as the one transport
  did, so every existing test and example stays green; `crates/clausters-server/tests/rt_safety.rs`
  stays green. **The packages move together**: `docs/schemas.md`,
  `docs/sample-clock.md`, both clients' builders and the GUI host's head
  clock, in the same pass.

- ✅ **T8 — The transport plays event lanes** *(done 2026-09-28; opened the same day by the
  user, out of the notes editor: "A piano roll with MIDI/OSC events should run on the server's transport, otherwise we would be duplicating functionality. A piano roll holds concrete data the same way audio clips do.")*. A take plays on the transport because its reader
  *follows* the position — a `BufRd` on `TransportPos` seeks on a locate,
  wraps on a loop and holds on a stop, with nothing sent per pass. A sequence
  of notes is data of the same kind and plays by other means today: the notes
  editor's playback (`crates/clausters-apps/PLAN.md`, `X3.8`) stamps every
  event on the transport's *clock* with `/sched_atTransport`, and the crate
  plans it again on every play, every locate and every edit — the transport's
  own work, done a second time outside it, and wrong across a loop, since the
  clock does not wrap. The notes regions of a multitrack (`X3.9`) would have
  been a third copy. Taken before `X3.9` sounds.

  **Why not on the audio thread, like a reader.** A node is built on the
  network thread (`Cmd::AddSynth` carries a `Box<dyn SynthNode>`), because the
  audio thread never allocates. A lane fires notes, and a note is a node, so it
  cannot be read out of a buffer by a UGen the way samples are.

  **The shape.** An **event lane** is a server resource bound to one
  transport and holding the events as data, in that transport's samples:
  **notes** — start, end, the def, its controls, the target and how it is
  released (`gate 0` or a free) — and **messages** — a position and an OSC
  message. A note is first-class rather than two messages because a lane plays
  again and again: its start and its release name one node, and that node's id
  is the server's to mint on each pass, never a number in the data. The
  **network thread feeds it**: on each turn it reads where the transport
  stands (it applies every locate, loop and end mark itself, and the engine
  publishes the position once per block) and schedules onto that transport's
  queue what of the lane falls within a lookahead ahead of the position, built
  into commands the way any bundle is — so what reaches the audio thread is
  exactly what `/sched_atTransport` delivers today, and the RT rules do not
  move. A locate, a change of loop, a wrap and a new lane drop what the lane
  queued and feed again from where the position is; across a loop the feed
  follows the wrap. What a lane queues is tagged with it, so a client's own
  `/sched_atTransport` bundles on the same transport are left standing, and a
  `/sched_clear "transport"` does not empty a lane for good. A stop freezes the
  queue and the governed group as it does now; a locate or a stop that goes
  back to a mark releases the notes sounding, as a DAW does on a jump; an edit
  keeps the release of every note already sounding.

  **What leaves the crate.** The notes editor's playback becomes the lane's
  data and the transport's verbs: `NotesPlayback`'s replan, the clock the
  client queries before every play, and the client's `_replan` go. The
  multitrack renders the notes of its boxes (`placed_notes`, in seconds of the
  multitrack) into one lane on its transport, sent again on every edit.

  **Open:** the commands' names and the data's wire form (JSON, as a def is
  sent, or a blob the core writes); the lookahead and the tick that keeps it
  fed — the loop polls at 10 ms while a client is subscribed and at 100 ms
  otherwise, and a rolling lane needs the first; how an NRT render feeds a
  lane, in step with its own blocks; whether a lane plays MIDI out once the
  server has a MIDI output.

  **Acceptance:** in an NRT render, a lane's notes start and end on their
  exact samples through a locate, a loop wrapped with no client in the loop,
  and an edit mid-pass; a note sounding at a locate is released; a client's
  `/sched_atTransport` bundle on the same transport sounds while the lane is
  fed and fed again; the notes editor plays, pauses, loops and locates with no
  message from the client but the lane's data and the transport's verbs;
  `crates/clausters-server/tests/rt_safety.rs` stays green. **The packages move together**:
  `docs/schemas.md`, `docs/sample-clock.md`, both clients' builders, and the
  crate's playbacks moved onto it in the same pass.

  **Shipped as that shape, with the open points settled** *(the user approved
  the proposal, 2026-09-28)*. `/lane_new <transport> <lane> <target>`,
  `/lane_set <lane> <json>` (`{"notes": [[start, end, def, {controls},
  "gate"|"free"]], "messages": [[position, address, args...]]}`, in samples)
  and `/lane_free <lane>`; a lane's id is the client's. One change from the
  shape above, found writing it: **the engine's lane queue is keyed by
  position, not by clock** — the network thread learns where the transport
  stands once per block, so feeding the clock queue would land every jump up
  to a block off. The engine cuts the block where the position reaches an
  entry, as it does at a loop's end, and a fired note's release goes onto the
  clock queue a note's length later. A jump (a locate, a pass going back to
  its mark) releases what the lanes sound; a wrap does not. A client's
  `/sched_clear` leaves a lane's releases standing. The feed keeps half a
  second built and the loop turns every 10 ms while a lane exists. What comes
  back unrun is forgotten on the network side, which fixed the same mirror
  leak for a cleared `/sched_atTransport` bundle. The acceptance's render is
  the pulled server (`crates/clausters-server/tests/lanes.rs`), where the feed and the blocks run in a
  fixed order and every assertion is a sample; an NRT **score** has no lanes.
  The notes editor's playback is a lane on its transport (`update` replaces
  `replan`, and no client asks for a clock), the multitrack's notes regions
  are one lane in its tracks' group (`MultitrackPlayback::notes`, the editor's
  `notes` verb), core ABI 76. `docs/decisions.md` has the reasoning and
  `crates/clausters-server/examples/event_lane.py` is the manual test.

- ✅ **T9 — A lane plays MIDI** *(done 2026-09-29; opened 2026-09-29 by the user, out of
  `crates/clausters-apps/PLAN.md` `X3.10`: the server's lanes should carry
  MIDI messages before anything is recorded (`T10`), since `T8` made the lane
  and this completes it)*. The server plays MIDI: a message that reaches its
  live input (`crates/clausters-server/src/midi/live.rs`) goes through `CmdTranslator::translate_midi`
  and sounds the instrument `/midi_bind` put on its channel. A lane cannot
  carry one. The crate's `notes_playback::data`, which writes a lane from an
  `EventSequence`, drops every event of type `"midi"` -- and every one of type
  `"osc"`, on the assumption that an OSC event is addressed to another
  application. *(First written as a MIDI output on the server, corrected the
  same day by the user: the server plays the MIDI it is given, live or on a
  lane, and sends it to no one, so no output is opened.)*

  **The shape.** A lane's MIDI message goes down **the live input's own
  path**, at its position on the transport, as though it had reached the port
  then: the channel's binding says the instrument, where it goes, whether it
  gates and which controls a message moves, and the same builders make the
  same commands. Only *when* differs: the lane builds its entries ahead of the
  position, as it builds its notes, so what the live path decides at the
  moment a message arrives the lane decides from its own data.
  - **A note-on and its note-off are one lane note**, paired on the lane's
    data by channel and key, so a note is built once with its release, and a
    jump, a stop or an edit releases it as it releases a lane's synth. Its id
    is the MIDI range's, as a live voice's.
  - **An expressive message** (a controller, bend, pressure) moves the lane's
    own voices sounding on that channel at its position -- read off the
    pairing, not off what sounds when it is built. A live voice and a lane's
    voice do not reach each other.
  - **A program change** chooses the instrument of the lane's notes after it on
    that channel, from the binding's programs, and changes no binding.
  - **An `"osc"` event is a lane `message`**, run by the server as written.

  **Also:** "A lane's data is written twice and typed nowhere" (Found by
  use) is fixed here, since the MIDI list is a third shape to write twice.

  **Acceptance:** a lane's MIDI notes start and end on their samples through a
  locate, a loop and an edit mid-pass, released on a jump; a controller on the
  lane moves its sounding voices; a note on an unbound channel sounds nothing
  and fails nothing; the notes editor and a multitrack's notes regions play a
  sequence's `"midi"` and `"osc"` events. `docs/schemas.md`, both clients'
  builders' docstrings and the crate's `data` move in the same pass.

  **Shipped as that shape.** `/lane_set` takes a `"midi"` list,
  `[[position, byte, ...]]`. The server pairs a lane's MIDI when it is set
  (`lanes::midi_events`): a note-on and its note-off become one entry with a
  release, a note-on the lane never turns off is not played, and a
  controller names the lane's voices it reaches by a key the feed resolves to
  the nodes built in the current generation. The translator's live path and
  the lane share its builders (`midi_voice`, `midi_release`,
  `midi_expression`), so a lane's voice is the same command a live note-on
  makes. The lane's data is one type, `clausters_core::lane::LaneData`, which
  the crate writes and the server reads. `crates/clausters-server/tests/lanes.rs` has the MIDI cases
  and `crates/clausters-server/examples/event_lane.py` plays the figure as MIDI too.

- ⬜ **T10 — The server records MIDI** *(opened 2026-09-29 by the user, moved
  out of `crates/clausters-apps/PLAN.md` `X3.10`: recording is a feature of
  the server, not of the notes editor; taken after `T9`)*. The server records
  MIDI as it records audio: the bytes that reach its input
  (`crates/clausters-server/src/midi/live.rs`), each stamped with the transport's position, into a
  buffer of its own, written to a Standard MIDI File that the session then
  points at as a source (`crates/clausters-document/PLAN.md`, Found by use,
  "A MIDI file and an OSC score are sources, as a sound file is, and a
  session holds neither inside"). Stamped on the transport, a MIDI take is on
  the same axis as an audio take recorded in the same pass, and in step with
  it. It is not written into an `EventSequence`: a sequence is the client's
  representation of what a `Timeline` renders to, not something recorded. A
  client's `MidiFunc` still receives MIDI in real time; it does not write the
  take.

  **Open:** the commands (arm, stop, write); the tempo the file's ticks are
  written against; whether one recording spans several input ports.

Open, not blocking:

- ✅ **T2 — `/transport_set`'s grid origin on the transport axis.** With a group
  bound, `originSample` is still read on the device axis, so the grid slides by
  the frozen total across a pause. Needs the grid semantics re-derived; no test
  pins it today.

  **Closed 2026-10-07, decided with the user: the grid stays on the device
  clock, and the premise was wrong.** Re-deriving it showed that a grid held
  through a governed pause cannot be the grid this command was made for.
  `/transport_set` leaves the transport stopped at 0, and clients phase-align
  on the grid before anyone plays: `clients/python/examples/transport/sync.py` defines it,
  binds a group and has two clients quantize onto it while the transport is
  stopped, and only then plays. A grid that held with the transport would hold
  those notes forever. So the grid is a **metronome**, and the "slide" this
  entry named is what a metronome does while the music pauses. What stops and
  locates with the music already exists: the transport's **position**, in
  beats and in samples, which is where a client reads the music's bar.

  Two other designs were worked through and left. The transport's own clock
  does not run at all without a group (the engine is only told to roll a
  governed transport), so a grid on it froze every ungoverned use. And a third
  clock — the device's minus the time stopped with a group bound — fixed that
  and still froze `sync`, while adding a number that agrees with the
  transport's clock only some of the time.

  Pinned: `two clocks join one grid and land on the same bar`
  (`clients/web/tests/seq-ws.test.ts`, against a real server) now binds a
  governed group and leaves the transport stopped, as `sync` does, and both
  notes still land on the bar. The contract is written where the grid is
  documented: `docs/schemas.md`, `docs/sample-clock.md` and both books'
  transport pages.
- ⬜ **T3 — Classification is once, at drain.** A bundle scheduled before
  `/transport_group` binds stays on the device queue even if its target becomes
  governed. Documented; re-classifying would mean rewriting a queue on the audio
  thread.
- ✅ **T4 — `bundle_is_governed` re-resolves the group per message.** A linear
  `find` per targeted message, bounded and allocation-free but `O(messages x
  nodes)` worst case inside a block budget. Hoist the group's index.

  **Done 2026-08-26, and the fix is smaller than the entry expected.** Writing
  it turned up what the second scan was actually for: `is_descendant_of` walks
  up the parent links comparing **ids**, so the ancestor's *index* is never
  read — the second `find` established one bit, *is the ancestor in the tree at
  all*, at the cost of a full scan of the slots, once per targeted message.
  So there is no index to hoist. `NodeTree::anchor` asks that question once and
  `is_under` walks from the target alone, which halves what a bundle costs
  (`2M` scans become `M + 1`); `is_descendant_of` is now the pair, so there is
  one walk and not two.

  **What is left un-hoisted, deliberately:** the target's own `find`. That is
  the `O(messages x nodes)` term the entry names, and removing it is an
  id → slot index, which is a data structure rather than a fix. Nothing on the
  path to the complete example sends bundles wide enough to want it.

  **The network-side twin needed nothing**: `packet_targets_group` walks the
  mirror, which is keyed by id, so it never had the scan.

  Tests: the anchored walk agrees with the plain one for every id including an
  unknown one, and an anchor is not fooled by its group being freed and the slot
  taken by another — the case an index would have got wrong, which is why the
  anchor holds an id.

## S track — Synthesis-engine infrastructure completion (the substrate for future UGens)

Section added 2026-07-01. The base UGen set and the node/bus/def machinery are in place, but before *growing the UGen library* we finish the **substrate** every future UGen leans on, so that adding a UGen later is a self-contained job (a `process`, a registry entry, a test) with no engine surgery. Everything here is deliberately **infrastructure, not a UGen catalog**: the concrete DSP UGens it enables — the demand family (`Dseq`/`Dseries`/`Dwhite`), `FFT`/`IFFT` and PV_* processing, the table oscillators (`Osc`/`VOsc`/`Shaper`), more filters — land afterwards as loose items (per "Future milestones (M9+)"), each cheap once the substrate exists. Like the F fork, S coexists with the M line and does not replace anything; the pieces are largely independent (S1 enables S2's `ir` controls and the future demand/FFT UGens; S5 depends on S6's `/buffer_gen` slot but is written together; S4 pulls `/node_run` in from S6 because it is tied to pause semantics). Canonical scsynth lists below were **verified against the SuperCollider source** (`server/plugins/UnaryOpUGens.cpp`, `BinaryOpUGens.cpp`, `HelpSource/Classes/Done.schelp`) on 2026-07-01.

**Design stance — compatibility of *model*, not literal copy.** These milestones take scsynth's set as the reference for *completeness* (so no capability is missing), but each is free to adopt a **better implementation consistent with what Clausters already built** where scsynth's is weak — the same "conceptual, not binary, compatibility" principle stated at the top of this plan. scsynth carries historical warts we need not inherit: the clearest example is the plugin-command pair **`/server_cmd`/`/node_ugenCmd`** (S6), which were never cleanly designed (untyped, ad-hoc argument blobs, per-plugin conventions with no schema) — where we need that mechanism we should design a **typed, discoverable** command surface fitting our JSON/OSC conventions rather than reproduce the loose original. Likewise favor our existing machinery (auto-ordered groups, the bus analysis, the network-thread pre-build, the versioned ABIs) over scsynth idioms that predate it. Each S milestone should call out, when it deviates, *why* and *what* it does instead.

- ✅ **S1 — Calculation rates (`ir`/`kr`/`ar`/`dr`) as a first-class property** *(done)* — the output/control rate becomes an explicit, compiler-validated property (with an `ir` init pass and a minimal `dr` demand-pull driver), the substrate for the demand and FFT families.

- ✅ **S2 — Typed controls: `tr`, `lag`/`varlag`, and scalar (`ir`) controls** *(done)* — control types the def author chooses: trigger controls reset after one block, lagged controls insert a shared `Lag` at compile time, `ir` controls freeze at init.

- ✅ **S3 — Special-index operator UGens (`UnaryOpUGen`/`BinaryOpUGen`) + `MulAdd`/`Sum3`/`Sum4`** *(done 2026-07-02)* — two generic op UGens whose `op` is the operator's name over the full scsynth opcode set, computed by the shared `clausters-core` functions for bit-identical client/server parity; `Add`/`Sub`/`Mul`/`Div` stay as aliases.

- ✅ **S4 — Complete the done-action set + `/node_run` (resume) + non-terminal pause** *(done 2026-07-03)* — the full 0–15 `DoneAction` enum (the relative-node actions resolved on the audio thread) and `/node_run`, which makes `PauseSelf` non-terminal.

- ✅ **S5 — Wavetable & table-generation infrastructure (`/buffer_gen`) + the table oscillators** *(done 2026-07-03)* — `/buffer_gen` (`sine1`/`sine2`/`sine3`/`cheby`/`copy`) fills buffers through the immutable-`Arc` NRT path, the scsynth interleaved wavetable format, and `Osc`/`OscN`/`VOsc`/`Shaper` as the first consumers.

- ✅ **S6 — Complete the scsynth OSC command set** *(done 2026-07-03)* — the missing node/group/bus/synth/buffer/def/scheduling vocabulary (`/node_setRange`/`/node_fill`/`/node_mapRange`, `/group_head`/`/group_tail`/`/node_order`, `/bus_setRange`/`/bus_getRange`/`/bus_fill`, `/synth_get`/`/synth_getRange`, `/buffer_close`, `/def_load`, `/sched_clear`) plus a typed `/server_cmd`/`/node_ugenCmd` surface and `/server_errorMode`.

- ✅ **S7 — Boot-time server configuration (audio I/O channels + every pre-allocated pool)** *(done 2026-07-03)* — `--inputs`/`--outputs` (with a real audio-input path via a second cpal stream) and `--max-nodes`/`--max-buffers`/`--max-graph-children`/`--max-ugen-inputs`, chosen at boot and reported in `/server_query`.

- ✅ **S8 — FFT/IFFT and the spectral (`fr`) chain** *(done 2026-07-03)* — `FFT`/`IFFT` + `PV_MagAbove`/`PV_MagBelow`/`PV_BrickWall`, one frame per hop; the transform planned at init (allocation-free per-hop), synth-private spectral scratch, and `/node_ugenCmd` as the live window-swap channel (S6's first consumer).

- ✅ **S9 — Side-effect UGens (no `Out` required)** *(done 2026-07-03)* — `SendTrig`/`SendReply`/`Poll` sending replies out the RT-safe reply FIFO; a valid def may contain only side-effecting UGens (the client relaxation is C19). Write-only UGens (`Out.kr`/`DiskOut`/`RecordBuf`) deferred with the buffer/streaming work.

- ✅ **S10 — Finite-resource registries: every id allocator recycles** *(done 2026-07-16)* — one shared `clausters_core::registry::Registry` (occupancy map, FFI-exposed) behind the server's `/synth_new -1` auto range, the MIDI voice range and the GraphDef private buses, and behind the Python client's node/bus/buffer allocators; the node-id space partitioned by `NodeIdPartition::from_max_nodes` (replacing the 2M/3M counters); client ids recycled via `/node_end`, engine rejections broadcast `/fail` with the id appended so nothing is lost; NRT node ids unbounded by design. Rationale in `docs/decisions.md`.

- ✅ **S11 — Hop-phase staggering for spectral chains** *(done 2026-07-18)* — de-align the hop blocks of concurrently running `FFT` chains so their transform spikes spread across blocks instead of stacking on one. Today every chain instantiated on the same block hops on the same block: the `bench` spectral section measures 32 aligned 1024-point chains at ~8% average load but ~65% of the block budget on the hop block — the sawtooth worst case. The fix is an initial `since_hop` offset chosen **deterministically per instance** (derived from the node id modulo the hop, quantized to blocks), so RT and NRT stay sample-identical for the same score and a given chain's own analysis is untouched (the hop was always quantized to the processing slice; only *which* block a chain first fires on shifts). Acceptance metric: the peak-block column of `crates/clausters-server/examples/bench.rs` at 32/128 voices approaches `avg + one chain's pair cost`. Document the stagger in `docs/architecture.md` (spectral section) and note the determinism rule next to the NRT sample-identity invariant.

- ✅ **S12 — Editing does not go through the pool, and its verbs are three** *(opened 2026-08-15 as "a buffer write costs the samples written", rewritten twice the same day as the premise under it was rejected and then narrowed. The measurement is kept because it is what makes the decision safe rather than merely tidy; the earlier shapes are kept named because a milestone that quietly changes its mind teaches nothing.)*

  **The decision.** An editor's write never travels through the real-time server's buffer pool. The RT pool buffer stays immutable and replaceable exactly as `crates/clausters-server/src/dsp/buffer.rs` says, `Osc`/`VOsc`/`Shaper`/`Conv` are untouched, and the question this milestone was opened to answer — what those four do when pointed at a writable buffer — **does not arise here at all**. (It does arise in S14, which is a different subject: recording, not editing.)

  **What was measured** (2026-08-15, release build; writes of 1 ms fired back to back, closed by one `/server_sync`, so the NRT queue's serial rate is what shows):

  ```
  10 s stereo    3.7 MB     2.26 ms per write     443 writes a second
  1 min stereo  22.0 MB     6.49 ms per write     154 writes a second
  5 min stereo 109.9 MB    33.84 ms per write      30 writes a second
  10 min stereo 219.7 MB   62.88 ms per write      16 writes a second
  ```

  Linear in the **buffer** (~0.29 ms per MB) and flat in the span, three quarters of it allocating and faulting in a second take rather than the memcpy. What that establishes is narrower than what it was gathered for: 33.8 ms is a defect **only under the assumption that every stroke writes the take**, and that was never the architecture. The working copy leads while the session is open (`crates/clausters-document/PLAN.md`, O8), a stroke is heard against the span it just drew (a scratch buffer that length sustains 443 writes a second, the route D4 already takes), and a take's pool buffer is replaced whole **once, on confirmation** — where replacing it whole is the correct operation and not a cost, because the material changed.

  **The placement rule this settles, and it is sharper than "does it need a graph":** what separates the two families of edit operation is **whether it has a timeline**. No timeline — arithmetic over samples — is `clausters-core`. A timeline — anything that runs a UGen graph, where an envelope evolves and a delay line fills — is the NRT engine (S13).

  **The three verbs**, all `clausters_core::edit`, pure over `&mut [f32]` plus a channel count, no I/O, and **none of them a new algorithm**:

  | verb | over the span | what it covers | what it reuses |
  |---|---|---|---|
  | `gain` | `from`, `to`, `shape`, `curve` | constant gain (`from == to`), fade in/out, silence, each half of a crossfade | `clausters_core::envshape::shape_value`, the SC shape numbers `EnvGen` and the BPF editor already speak |
  | `replace` | the samples | the pencil stroke (D2), the paste (D4) | a `copy_from_slice` |
  | `reverse` | — | a reversed span | a frame-wise reverse in place |

  `normalize` is not a verb: it is a measurement (the A track's descriptors) followed by `gain`.

  **Where they live is not who runs them, and the distinction is what keeps a settled decision true.** The verbs are `clausters-core` because that is where a shared algorithm lives (the placement rule), but the **performer is the NRT server** (S13): a client drives that mode, it does not call the core functions itself. So *one place performs audio processing and it is the server* (`crates/clausters-document/PLAN.md`) holds as written and needs no amendment — it speaks to who performs, not to which crate the code sits in. An earlier shape of this milestone had the Python client calling the verbs through the FFI with no server involved, which would have made two performers; S13 is what removed the reason to.

  **Two boundaries that follow.** **Length changes stay in the arrangement**: deleting a span from a flat file rewrites everything after it, while a placement already expresses "this part does not sound", undoably, which is what D4 shipped — so cut and paste stay placements and destructive length change is left to consolidate/bounce, which is a whole-file rewrite by nature. And **each verb answers which span it wrote**: the server holds no session state, so it cannot touch a peaks cache, and whoever owns the cache calls `peaks::update_range` over the span it was told about (D1's half of the same sentence).

  **Acceptance:** a drawn edit on a five-minute take is applied, heard and confirmed with no pool buffer written per stroke; the RT read path is untouched byte for byte, with `crates/clausters-server/tests/rt_safety.rs` and the golden renders saying so; each verb is tested against a hand-computed span; and no verb duplicates an algorithm the core already has.

  **What shipped** *(2026-08-15)*. `clausters_core::edit` — `gain` (taking a `Fade`, which is `constant` or `from_to` along an `envshape` shape number), `silence`, `replace` and `reverse`, pure over `&mut [f32]` plus a channel count, spans in **frames**. On the wire, `/buffer_gain` and `/buffer_reverse` join the writing family as ordinary NRT jobs, chained like `/buffer_setRange` so a batch of edits composes in flight rather than each starting from the pre-batch contents; `replace` needed no command of its own, being `/buffer_setRange` already, and `silence` is `gain 0 0`. Python (`Buffer.gain`/`fade`/`silence`/`reverse`) and TypeScript builders in the same commit, `docs/schemas.md`, and `clients/python/examples/buffers/buffer_edit.py` grown a second half: the same edits said rather than performed client-side, with no samples crossing the wire.

  **No verb duplicates an algorithm**, which was the acceptance clause worth checking rather than asserting: `gain` rides `envshape::shape_value` (what `EnvGen` plays and the breakpoint editor draws, so a fade is the same curve everywhere), `replace` is a `copy_from_slice` and `reverse` a frame-wise swap. `normalize` stayed out for the same reason it is not a verb.

  **Where the acceptance over-reached.** Its first clause — *a drawn edit on a five-minute take is applied, heard and confirmed* — is D1 and D2's, not this milestone's: there is no pencil yet to draw one. What this can and does claim is the half it owns: no pool buffer is written per stroke because no verb writes a take at all, and the RT read path is untouched, which `rt_safety` and the golden renders pin.

  **Two things the work turned up, both recorded where they belong.** The command table is searched with `binary_search`, so `/buffer_gain` filed in the wrong place made it **unreachable** while reading as wired up — now guarded by a sortedness test that would have caught it. And ten examples play routines on a clock nobody starts (`clients/python/PLAN.md`, "Found by use"), which is how the new example cell came back silent.

- ✅ **S13 — The NRT server takes operations on demand** *(opened 2026-08-15, out of the editing pass; proposed by the user as "add an interactive mode to the NRT server, and do not touch the RT one")*. Today the NRT side is a batch renderer: it takes a score and returns a file, start to end. Editing needs the same engine to execute **operations on demand**, because interaction is not predictable — it answers a document, not a timeline.

  **It is its own mode, not a variant, and that is the point.** Almost every part is reused — the engine, the translator, the OSC surface, and the pulled door `crates/clausters-server/src/embed.rs` already proves (`server.step()` before each `engine.process_block`, the caller owning the clock and the sink) — and the *semantics* are not shared. Treating it as a mode from the start is what keeps the reuse from turning into a mode that is secretly two.

  **The shape, settled 2026-08-15 before writing any of it**, because the sentence this milestone opened with — *it goes on the `Renderer`'s path* — turned out to name the wrong half. Reading the two: the `Renderer` has the synchronous execution and its own `now` and has **no** transport, no clients, no `/done`/`/fail`/`/server_sync`, and only part of the `/buffer_*` family; `OscServer` has all of that, and `crates/clausters-server/src/embed.rs` already drives it with no audio device (`Segment::in_memory()` + `OscServer::headless` + an `IpcPeer` at each end, `step()` before each block, the caller owning the clock and the sink). So the **front is embed's shape**, reused rather than rebuilt, and *batch* is honoured where it belongs — in the **operation**, which is a closed render over the instance's engine. `render.rs` stays the score's path, untouched, and so does the RT server, which is the proposal's own condition.

  **Two mechanics that fall out of having no clock.** Between operations the engine must still *apply* what arrived (a `/buffer_alloc` installs through the engine's command FIFO) without time passing, so the engine grows a `drain` that is `process_block`'s first two steps and none of the rest. And the operation runs on the **instance's** engine rather than a fresh one per operation: a fresh engine would have to be handed the loaded buffers, which is the 110 MB copy this whole line of work exists to avoid.

  **There is no server clock.** Nothing advances between commands and nothing is scheduled against a running `now`. Two tiers follow, and conflating them is the mistake available here: **buffer-editing commands have no timeline in any sense** (S12's verbs — applying a gain to a span is not an event at an instant), while a **render operation does have one internally** (an envelope evolves, a delay line fills) — a self-contained score starting at 0 and lasting the span. What it deliberately does not carry, named so it does not drift in: interactive *generation* against a clock, which is a plausible thing and is not editing.

  **Determinism is of process, not of time.** A batch render is deterministic because it runs start to end; an interactive session cannot be, and does not need to be. The invariant is instead that **the same operation over the same material yields the same samples it would yield expressed in a score and rendered in batch** — which is exactly testable, with T1's own acceptance technique (cut the span out of a batch render and assert it equals the operation's output sample for sample, over a seeded-noise def, deliberately not block-aligned).

  **In this mode a pool buffer is mutable**, because there is no audio thread: the immutability contract is an RT rule, not a property of a buffer. So the loop is load (`/buffer_allocRead`), edit in place, write (`/buffer_write`), and the loaded buffer *is* the session state — which is right for an editing instance and would be wrong for a live server. From the T track it reuses the type discipline (`crates/clausters-server/src/server/clock_axis.rs`: the axis is a type) and not the clock, there being no second axis where there is no first.

  **Open, to design once the mode runs (noted 2026-08-15 by the user): can it move the samples through shared memory instead of over the wire?** Bulk crosses as OSC blobs today (`/buffer_setRange` in, `/buffer_getRange.reply` out), which is the right trade for a client filling a buffer once and the wrong one for an editing session, where the material is a take and the operations are many. Two directions and neither is chosen here: **share the segment** — the machinery exists at both ends (`--shm` for a local RT client, `Segment::in_memory()` for embed), so an out-of-process session could carry samples the way control buses already travel; or **share the file**, which is sharper if it holds — the client owns the working copy and the GUI host already maps it read-only, so if the session maps the same file there is no data to move at all, only a span to name. The second would make the first unnecessary for editing and not for anything else.

  **Acceptance:** an operation applied interactively is sample-identical to the same operation in a batch score; the RT server's sources are untouched; the mode builds and runs **without `realtime`/cpal**; a `/server_quit`-shaped end and the absence of any scheduling surface are both tested.

  **What shipped** *(2026-08-15)*. `server::nrtsession::NrtSession` is the mode, on embed's front exactly as decided above; `Engine::drain` is the one thing the engine had to grow, and `/buffer_render bufnum frames` is the operation on the wire — `/buffer_gen`'s sibling, generating into a buffer by playing rather than by formula. The command's two halves stay apart on purpose: the server owns the message (parse, validate, answer) and the driver owns the engine, so it queues an `OfflineRender` the driver performs between commands, which is the NRT queue's own shape with the driver in the worker's seat. It is legal only where something owns the clock (`enable_offline_renders`), and a real-time server fails it rather than approximating — pinned by a test, since that is the half of *the RT server is untouched* that code can check.

  **Where the acceptance was wrong, and it is the milestone's own sentence.** *"The absence of any scheduling surface"* is not a property this mode has or should have: a **timetag is meaningful here**, because an operation *is* a score — a bundle inside its span lands at its exact sample exactly as the batch renderer places one, and one past the end waits for the next operation instead of firing. What the mode lacks is a clock that moves **on its own**, which is a different claim and the one that is now tested from both sides (an operation split around thirty-two settles joins seamlessly; a bundle beyond the span cannot be made to fire by settling).

  **What is honestly not "untouched", stated rather than glossed:** the RT server's *behaviour* is unchanged and `crates/clausters-server/tests/rt_safety.rs` and the golden renders say so, but four additive things reach shared code — `Engine::drain`, `OscServer::set_seed`, the offline-render request pair, and the `/buffer_render` dispatch row. None of them runs in a real-time server; all of them compile into it.

  **What is left, and it belongs to the client leg rather than here**: no client can reach a session yet, since the front is the in-process ring and nothing exposes it over a socket or through the C ABI. So `/buffer_render` has its reference in `docs/schemas.md` and **no Python or TypeScript builder** — the packages move together, and a builder for a command no client can send would be a shape nobody exercises. Its example is in the same position, which is why this milestone closes without one. Both land with whatever first gives a client a door to a session, which is S12's verbs needing one.

- ✅ **S14 — Every pool buffer is writable, and the write-side UGens exist** *(opened 2026-08-15; the gap S9 deferred and `crates/clausters-server/src/dsp/buffer.rs` and `crates/clausters-server/src/dsp/delay.rs` have both been naming in their module docs since)*. In SuperCollider you allocate an empty buffer, zero it and use it for anything — record into it, delay through it, loop it, save it. Here you cannot, because a pool buffer is immutable once built, and **that is a missing capability rather than a design stance**: it is exactly the completeness the S track exists to reach.

  **What it unblocks, named:** `RecordBuf` and `BufWr` (write-side UGens, deferred in S9 for want of this substrate) and the `BufDelay*` family `crates/clausters-server/src/dsp/delay.rs` says it cannot have (*"a delay over a pool buffer would have to mutate one"*) — the looper, the multi-second delay whose contents you want to inspect, resample or save. Long delays themselves already work through synth-private lines allocated at `build`; what is missing is only the **shared** case: record here, read there.

  **The shape as it was written, and how it changed.** It said: *writable is declared at allocation and is not a promotion — a buffer allocated for recording is not a wavetable and never becomes one, so the four per-block `data()` readers (`Osc`, `VOsc`, `Shaper`, `Conv`) refuse a writable buffer by name at build, while an ordinary buffer keeps flat `[f32]` storage and today's speed.* **That is not what shipped** *(the policy was changed 2026-08-16, by the user, before the substrate was written)*: there is no kind, no declaration and no refusal — every buffer is writable. See "What shipped" for the measurement that made the choice cheap and for the promise the type system could not have kept anyway.

  **The reader cost is measured, not assumed** (2026-08-15): with `[AtomicU32]` storage and relaxed loads, an interpolated stereo read of 64 frames goes 145 → 150 ns (**+3%**), and **+0%** on a wavetable-shaped read of a table hot in cache. Per-element atomicity means no sample is ever read half-written; what a reader can see is a mixture of old and new across a span, which is scsynth's own semantics for exactly this case and is what a looper crossing its own write head has always sounded like. **The concurrency is real and is why the immutable design was chosen** — the DSP workers process in parallel — so the writable kind must state what it guarantees (per-element atomicity, no ordering between elements) rather than inherit an invariant it breaks.

  **Three scsynth commands turned up missing on the same pass and are S15's**, not this one's: they need no writable buffer and this needs none of them. `LocalBuf` is named here deliberately and **not** claimed as missing: its main scsynth use, the spectral chain's own buffer, is already synth-private scratch here, and its other uses are covered by the private delay line — so it needs a decision, not an implementation.

  **Acceptance:** a synth records into an allocated buffer while another plays it, and the loop sounds; a `BufDelay` over a pool buffer runs; `crates/clausters-server/tests/rt_safety.rs` stays green and the read path's cost is reported; `Osc` pointed at a writable buffer refuses by name.

  **Taken in the same pass as S17** *(decided 2026-08-16)*. They are one pass over the buffer-UGen surface and over `crates/clausters-server/src/dsp/buf.rs` in particular: this one changes how a reader **reads** (the `[AtomicU32]` storage whose +3% is measured above, in `read_lin` and `PlayBuf::process`), and S17 changes what a reader **is** (a trigger, a cue point, a done action) in the same two functions. Doing them apart means editing that path twice and measuring it twice.

  **What shipped** *(2026-08-16, over four commits)*. The substrate first: `Buffer`'s storage is `Vec<AtomicU32>` holding `f32` bits, the shape (`frames`, `channels`, `sample_rate`) is settled at allocation and immutable, and every read goes through one relaxed-load door (`load`/`at`/`sample`) while `set_at`/`set_sample` take `&self` — the pool reaches the audio thread through an `Arc`, so the mutability has to live in the cells. `data() -> &[f32]` is **gone**, which is what forced every reader to be looked at: the wavetable path, the oscillators and the convolver now read `&[AtomicU32]` and are bit-identical (the goldens say so). Then `RecordBuf` and `BufWr`, then the nine `BufDelay*`/`BufComb*`/`BufAllpass*` rows over one implementation parameterised by `Storage { Private, Pool }`.

  **The refusal was dropped because it could not have been kept, and dropping it removed more than it cost.** A `bufnum` is a **runtime control** — `Osc`'s buffer is an input, not a static field, so "refuse a writable buffer at build" was never available to the compiler; what it could have done is fail at instantiation, or read silence, which is a worse answer than reading the samples that are there. Gone with it: the kind, the wire flag on `/buffer_alloc`, the branch in every reader, and a second storage layout to keep working. What remains is one sentence in the reference — contents are mutable, the shape is fixed.

  **The cost, measured before the decision and unchanged after** *(the acceptance's "the read path's cost is reported")*: an interpolated random read (`PlayBuf`/`BufRd`, 64 frames) goes 145 → 150 ns, **+5%** — 12 ns a block a reader, about a **thousandth of a percent** of a 64-frame block's budget at 48 kHz — and **+0%** on the three other shapes (sequential, wavetable, the convolver's kernel), where the loads vectorise as before. What is bought is per-element atomicity: a reader crossing a writer sees old samples and new, never half of one.

  **The example is `crates/clausters-server/examples/buffer_writing.py`**, and what it had to solve is worth recording: a looper that does not click. Two rules make it silent — the recorded phrase is windowed to zero at the loop's seam and every change of what is written is made *during* that silence, where a change can store no edge; and the reader is started **after** the writer at the same rate, so it trails it by a fixed distance and never crosses the write head. The stored loop was then measured rather than trusted: the largest sample-to-sample step in the buffer is a fifth of the material's own steepest slope, and the seam is exactly zero.

- ✅ **S15 — `/buffer_fill`, `/buffer_readChannel`, `/buffer_allocReadChannel`** *(opened 2026-08-15, found while writing S14 by reading our `/buffer_*` set against scsynth's `b_*` one; recorded rather than folded in, because none of the three needs a writable buffer and S14 needs none of them)*. Those three names are **the** names — the `/<resource>_<action>` rule applied to scsynth's `/b_fill`, `/b_readChannel` and `/b_allocReadChannel`, following the `/buffer_read`/`/buffer_allocRead` pair that is already there. S6 declared the scsynth command set complete and these are not in it, and are not recorded as deliberate omissions anywhere either — which is the part worth fixing beyond the commands themselves, since an undocumented gap in a set declared complete is what makes the next reader trust the wrong thing.

  - **`/buffer_fill`** — fill ranges with a value (`[start numSamples value]...`), scsynth's `/b_fill`. It is the sibling of the `/bus_fill` that **S6 did ship**, so the asymmetry is inside one milestone. It needs nothing new: an NRT job shaped like `Set` with a value instead of a run, laid into the copy that replaces the buffer whole, exactly as the writing family already works. It is also an editing verb — silencing a span is a fill with 0 — so S12's `gain` and this overlap deliberately: one is the core function an editing mode calls, the other the wire command any client has.
  - **`/buffer_readChannel`** and **`/buffer_allocReadChannel`** — scsynth's `/b_readChannel` and `/b_allocReadChannel`: read *selected channels* of a file, into an existing buffer or into a fresh one. This is how a stereo file's left channel is loaded into a mono buffer, and today it cannot be done at all: `/buffer_read` **fails** on a channel-count mismatch (`"channel count mismatch: buffer has N, path has M"`), so the only route is loading every channel and paying for the ones you discard. `read_audio` returns interleaved frames, so the selection is a de-interleave at the end of the existing path rather than a second reader.

  **The packages move together**, so each lands with its Python and TypeScript builders and its row in `docs/schemas.md`. **Acceptance:** a fill over several ranges in one message, including one past the end, which fails rather than clamping like the rest of the writing family; a stereo file's second channel loaded into a mono buffer and asserted sample for sample against the interleaved read; and `docs/schemas.md` listing the three, so the set is complete in the reference and not only in the server.

  **What shipped** *(2026-08-16)*. `NrtJob::Fill` beside `Set` rather than folded into it — a fill says how *many* samples it writes, and expanding it into a run would allocate the very thing it exists to avoid — chained like the rest so a batch composes. The two channel reads are **one argument on the existing arms**, not a second implementation of reading a file: `select_channels` de-interleaves after `read_audio`, and an empty selection is every channel, which is exactly what `/buffer_read` and `/buffer_allocRead` send. Both clients' builders (`Buffer.fill`, `Buffer.read_channels`, `Buffer.read_channels_into`; `fill`, `readChannels`, `readChannelsInto`), the reference, and a test file that writes a real stereo WAV and reads each channel back out of it.

  **Two calls worth recording.** The channel list is a **variadic tail, so the positions before it are required** rather than optional — with a tail there is no telling a `fileStart` from a channel index, which is the same reason scsynth fixes those positions. And the order is honoured with repeats allowed (`1 0` swaps a pair, `0 0` widens a mono file), because that is what naming channels explicitly is *for* and it costs nothing to permit; a channel the file does not have **fails**, since asking for the right channel of a mono file is a mistake worth hearing about rather than a silent track.

  **No new example, stated rather than omitted:** the three are variants inside families the examples already exercise, and what is genuinely new — a channel-selective read — is covered end to end by a test that goes through a real file on disk rather than a mock.

- ✅ **S16 — A buffer write can address one channel** *(opened 2026-08-16, found by the GUI's standalone editor: the destructive-edit path refuses a multichannel take by name, and this is the command it names)*. Every writing command in the family addresses **flat, interleaved** samples: `/buffer_set` takes indices, `/buffer_setRange` takes a start and a contiguous run. That is the right addressing for filling a buffer and the wrong one for editing a *channel* of one — one channel of a stereo take is a **strided** span, and there is no command for it. The editing verbs S12 shipped are frame-addressed and already take a channel count, so the gap is on the wire and not in the core.

  **What it blocks, concretely.** The GUI host draws a take and writes a stroke back into the very buffer it is drawing; with more than one channel it refuses, with that sentence, rather than writing the wrong samples or sending one message per sample — which is the shape this rule exists to avoid (a stroke over a few thousand samples as N messages is the encode the blob convention was introduced to kill). So a stereo take is drawable and not editable today.

  **The shape, to decide when it opens.** Either a channel argument on the existing writers (`/buffer_setRange bufnum channel start blob`, where the run is that channel's own frames) or a separate `/buffer_setRangeChannel` beside `/buffer_readChannel` — the family already spells a channel-selective variant that way once, which is an argument for the second and against growing an argument on a command whose current shape is in every client. Whichever it is, `/buffer_set` gets the same treatment, and the reply, the past-the-end failure and the NRT chaining are the family's as they stand.

  **Acceptance:** one channel of a stereo buffer is written and the other is asserted unchanged, sample for sample; a span past the end fails like the rest of the family; both clients' builders and `docs/schemas.md` carry it; and the GUI host stops refusing a multichannel take (`crates/clausters-gui/PLAN.md`, H4).

  **What shipped** *(2026-08-16)*. Two commands, `/buffer_setChannel bufnum channel [frame value]...` and `/buffer_setRangeChannel bufnum channel [frame blob]...` — the `*Channel` spelling the family already uses once (`/buffer_readChannel`), with the channel **before** the runs because the runs are the variadic tail and a tail cannot be told from a start. Positions are frames of that channel; one message writes one channel, like every other per-channel thing here.

  **Inside, it is not a second job.** `NrtJob::Set` now carries `SampleWrite { at, stride, values }`: the flat forms pass a stride of 1 and the channel forms pass the channel count, so the copy-and-swap, the batch chaining and the bounds check are the ones that were already there — one conversion at the parse (`frame * channels + ch`) and nothing downstream knows. A refusal speaks the unit it was written in: past the end reports **frames** for a channel write and samples for a flat one, and a channel the buffer does not have fails the way a channel a *file* does not have already did.

  **It closed the milestone that asked for it in the same commit.** The GUI host writes through `/buffer_setRangeChannel` and no longer refuses a multichannel take; `Intent::WriteSamples` grew a `channel` (serde-default, so an older document reads as channel 0), which is what makes an undo put back the channel it took. Found by using it: the clip's take body drew channel 0 alone, the monitor played channel 0 alone, and a drag that left its lane read the *neighbouring* lane's value — all three in the "Found by use" list of `crates/clausters-gui/PLAN.md`.

- ✅ **S17 — `PlayBuf` completes its set: `trigger`, `startPos`, `doneAction`** *(opened 2026-08-16, while deciding T5's shape; the gap itself is older and both `docs/schemas.md` and `crates/clausters-server/src/dsp/buf.rs` have been recording it in prose — "starts at frame 0", "neither has a trigger or done action yet")*. scsynth's `PlayBuf` takes `trigger` and `startPos` (re-cue to a frame on a rising edge) and a `doneAction` (what happens when a non-looping pass reaches the end). Ours takes none of the three, so a buffer player can be started and freed and nothing else — no cue point, no re-trigger, and a finished one-shot sits in the tree outputting silence until somebody frees it. That is the S track's own subject: a capability missing from a set declared complete.

  **It is not what an editor's playback rests on, and that is deliberate.** T5 puts the piece's position in the engine and has readers *follow* it, which is the DAW shape and the one a multitrack needs — many readers, one time. This is the other shape, where a reader carries its own position, and both are legitimate: a one-shot sample triggered from a pattern has no business consulting a transport. So this lands when the S track's completeness argues for it, not as a dependency of anything in the GUI.

  **Taken in the same pass as S14** *(decided 2026-08-16)*, which is the open S milestone this shares its code with: S14 changes how a reader **reads** (writable storage, and the `read_lin`/`PlayBuf::process` cost it measures) and this changes what a reader **is**, both inside `crates/clausters-server/src/dsp/buf.rs`. Apart, that path is edited twice and measured twice; together it is one pass, one benchmark and one row rewritten in the reference.

  **Acceptance:** a rising trigger re-cues to `startPos` mid-play; `doneAction` 2 frees the synth when a non-looping pass ends, with the whole set behaving as `EnvGen`'s; a looping reader ignores the done action; `docs/schemas.md`'s row and both clients' builders move with it. `BufRd`'s own row is untouched — it is phase-driven and has no position of its own to cue.

  **What shipped** *(2026-08-16)*. The three inputs, read the way `RecordBuf` reads its own: `done_action` block-scalar like `EnvGen`'s, the trigger read per sample and *before* anything else in the loop so a finished player can be re-cued, and `start_pos` taken at the first block and at every rising edge.

  **The order is ours and not scsynth's, deliberately.** They arrive **after** `loop` rather than in scsynth's `rate, trigger, startPos, loop`, because inputs are positional and the arity check counts them: inserting `start_pos` before `loop` would have left every existing call the right length and silently re-read its `loop` argument as a cue frame. Appending costs a divergence documented in the reference and buys a wrong call that fails loudly. It still moved the arity from 4 to 7, so every hand-written def JSON in the tree grew three constants; both clients' builders default them, which is why no example moved.

  **A cue point belongs to whoever carries a position**, and that is the line the milestone draws: `PlayBuf` and `RecordBuf` advance themselves and take the set; `BufRd` and `BufWr` are driven by a phase signal and have nothing to cue — re-cueing one means changing the signal that drives it. Said in `docs/schemas.md` and in `crates/clausters-server/src/dsp/buf.rs`, where the prose used to promise the gap instead.

  **Found while checking it by ear:** an `EnvGen` retriggered mid-envelope restarts from its **initial level** rather than gliding from where it was, so a re-cue is a step in two places at once (the reader's jump and the envelope's). `crates/clausters-server/examples/buffer_writing.py` therefore fires grains shorter than the gap between triggers, which puts both steps in silence — the general rule for anything driven by a trigger, and the reason the example says it out loud.

- ✅ **S18 — A buffer write writes the samples it names** *(opened 2026-08-17 by the user, reading S14 against the write path: "now, with the UGens that write into buffers implemented, a write of samples does not cost the whole buffer". The observation is exact, and what it turns up is a **correctness** defect as well as a cost one.)*

  **The substrate moved and the command set did not.** S12 decided the write path around a premise it stated in one sentence — *"the RT pool buffer stays immutable and replaceable exactly as `crates/clausters-server/src/dsp/buffer.rs` says"* — and S14 rewrote that very module one day later: contents are mutable, every sample is an atomic cell any thread may write, and `set_at` is the door. `BufWr` and `RecordBuf` write through it from the **audio thread** every block. But every OSC write still copies: `NrtJob::Set`, `Fill`, `Edit`, `Gen` and `Read` all do `to_vec()` → lay the run in → `Buffer::new` → install a fresh `Arc` (`crates/clausters-server/src/server/nrt.rs`), justified in a comment by an argument the other module already answered — *"the audio thread therefore never sees a half-written buffer (which a per-sample write could show it)"* — while S14's module doc states the opposite as the intended semantics, scsynth's own: *a reader crossing a writer sees some old samples and some new, never half of one*, and that is what a looper crossing its own write head has always sounded like.

  **What it costs, measured 2026-08-17** (release, in-process, so the wire is not in it; a 1 ms stereo run written into the middle of the take, and the same op over the whole buffer):

  ```
                    size     copy-and-swap    in place     1 ms run
  10 s stereo     3.8 MB          0.31 ms     0.0001 ms      x5,800
  1 min stereo   23.0 MB          2.45 ms     0.0001 ms     x60,000
  5 min stereo  115.2 MB         32.9  ms     0.0001 ms    x570,000
  10 min stereo 230.4 MB         65.7  ms     0.0001 ms  x1,100,000

  whole-buffer gain:   10 s 0.30 -> 0.44 ms (x0.7)   1 min 2.95 -> 2.63 ms (x1.1)
                      5 min 41.4 -> 13.5 ms (x3.1)  10 min 82.3 -> 26.9 ms (x3.1)
  ```

  Two readings, and the second is the one that settles the shape. The span write is **flat in the buffer** rather than linear in it — the cost stops being a property of the material and becomes a property of the edit, which is what the whole entry at the foot of this file was asking for. And the *whole-buffer* op, the case where a copy should win, is **3.1x faster in place** at real sizes because it allocates nothing and touches the data once instead of twice; it loses only at 10 s, where the copy is cache-resident. So there is no size at which the copy is the right trade beyond about a minute, and the reproduction of S12's own table (32.9 ms against its 33.84 ms) says the measurement is the same one.

  **The correctness half, which is new since S14 and is why this is not a tidy-up.** A copy-and-swap write **silently discards** whatever an in-place writer put there in the meantime: the job snapshots the buffer at parse (or at chain time), and installing its copy replaces everything a `RecordBuf` recorded between the two. That is now an ordinary pairing rather than a contrived one — a looper being overdubbed while the editor writes a fade, a take being recorded while a client sets a range. Nothing warns; the samples are simply gone.

  **What the milestone does.** The write jobs write **in place** through the cells: `Set`, `Fill` and `Edit` become span writes with no allocation and no swap, `Gen` and `Read` keep the shape and fill in place (only `AllocRead`, which *changes* the shape, still builds a new buffer — correctly, since that is a different buffer). The chaining machinery (`NrtChain`, the `base` fallback on `Set`/`Fill`/`Edit`) exists **only** because a copy starting from a stale snapshot erases its predecessors, so it goes with the copies: in-place writes compose by construction. Bounds are already checked at parse, so a write cannot fail halfway and there is nothing to roll back. Nothing on the wire changes — no command, no argument, no reply — which is what makes this an implementation and a *documented semantics* change rather than a protocol one.

  **What must be written down rather than assumed**, because it is the trade being taken: a reader crossing a write now sees old and new samples. That is S14's stated rule and scsynth's, and it is the price of the capability; it belongs in `docs/schemas.md` beside the writing commands, not only in a module doc. The one place it must **not** leak is the golden renders, which are single-threaded and therefore unaffected.

  **Acceptance:** the table above, re-measured through the wire (a client's write on a five-minute take costs what a write on a ten-second one costs); a test that a `RecordBuf` recording survives a concurrent `/buffer_setRange` on the same buffer, which fails on today's code; `crates/clausters-server/tests/rt_safety.rs` and the golden renders unchanged; and the chaining tests deleted with the chaining, their property now holding by construction.

  **Done 2026-08-17. Measured through the wire, which is the acceptance as written:**

  ```
                    size    before      after
  10 s stereo      3.8 MB   3.94 ms    3.28 ms
  1 min stereo    23.0 MB   8.18 ms    3.63 ms
  5 min stereo   115.2 MB  39.75 ms    3.35 ms
  10 min stereo  230.4 MB  73.15 ms    3.54 ms
  ```

  The cost is **flat in the material** rather than linear in it, and what is left is not the write: ~3.4 ms is the `/server_sync` round trip these writes are measured behind, identical in all four rows because the write itself is microseconds.

  **What shipped**: `Set`, `Fill` and `Edit` write through the cells and return `NrtAction::None` — nothing is installed, because the samples went into the buffer the engine already holds. The editing verbs keep the core pure (`clausters_core::edit` is `&mut [f32]`), so an edit reads its **span** out, applies, and writes the span back: one allocation the size of the edit, not of the take. Two tests pin the correctness half that only became reachable with S14 — a recorder's samples survive a queued write, and an edit touches its span and nothing else.

  **One thing this milestone got wrong before reading the runner, corrected rather than quietly dropped:** the entry said the chaining machinery goes away with the copies. It does not, and it should not. `NrtChain` re-bases a queued write onto whatever the queue last *installed* for that index, and jobs that install still exist — `/buffer_read` and `/buffer_gen` replace a buffer whole (a read may even change its sample rate, which is shape and cannot be written in place). So the chain stays, doing less: it serves the replacing jobs' successors, and the write jobs it re-bases now mutate rather than copy. Nothing in it was deleted, and the tests that cover it still pass unchanged, which is the evidence that the property it protects was never the copies'.

  **What the replacing commands keep**, said in `docs/schemas.md` beside the rest: a recording crossing a `/buffer_read` or a `/buffer_gen` is replaced along with everything else, because replacing the material is what those commands *are*. The race this milestone closes is the one where a command claimed to touch a span and took the whole buffer with it.

- ✅ **S19 — The material lives in the shared segment, and a local peer edits it without a message** *(opened 2026-08-17 by the user, naming the target: "that the GUI client can work as a standalone application, with on-demand NRT processing and RT as separate processes", and then the consequence — "for an embedded server or over shm this can even save round-trip messages")*. Depends on **S18**: it is the same write, moved across a process boundary, and doing it while the write path still copies would mean two designs.

  **What the segment carries today and what it does not** (`crates/clausters-server/src/server/ipc.rs`): the versioned header, the sample clock, the transport clock and position, the 1024 control buses **as the very words the engine reads** (a peer's write is live on the next block, no command involved), the audio taps, the audio-bus region, and the two OSC rings. **Buffers are not in it.** So the one kind of data an editor is made of is the one kind that still crosses as messages: a take reaches the host as `/buffer_get` replies or as a mapped *file*, and an edit goes back as a `/buffer_setRange` blob.

  **The saving is not the copy, it is the round trip.** A stroke today, in the embedded or `--shm` host that H3/H4 built: the host sends `/buffer_setRange` with the span as a blob, the server queues an NRT job, copies the take whole, installs it, and answers — and the host, holding its own picture of the material, must then reconcile the two. With the samples in the segment the host **stores into the cells** and there is no message, no blob, no job, no reply and no second copy to reconcile; the engine reads those cells on the next block. That is not a new contract, it is the one the **control buses already have**, quoted from the module's own docs, and S14 already paid its conceptual price: per-cell atomicity, no ordering between cells, a reader seeing old and new. A cross-process writer is the same contract as the cross-thread one.

  **What stays a message, and this is the line the milestone draws:** everything with semantics beyond the samples — `/buffer_alloc` and `/buffer_free` (shape and lifetime), `/buffer_render` (S13: a graph, a timeline), `/buffer_read`/`/buffer_write` (the disk), and anything the server must order against other commands. What stops being a message is the sample traffic in both directions, including the **reads**: a peak pyramid is built over the mapped region rather than over a fetched copy, which deletes the largest data path the editor has.

  **What has to be decided, and none of it is decided here.** *The backing*: `Buffer` owns a `Vec<AtomicU32>`, so a shared one needs a second backing (a mapped region) behind the same cell API — the shape stays immutable, which keeps the directory simple. *The allocation*: an arena inside the segment, or a region per buffer, plus a `bufnum -> region` directory a peer resolves. *The lifetime*: a buffer freed while a peer has it mapped, which the control buses never had to answer because they are fixed and eternal. *The ordering*: with no message there is no `/done` and no `/server_sync` to sequence a write against a command that depends on it (write, then `/buffer_write` to disk), so either a per-buffer generation counter in the segment or a stated client-side rule. *Who may write*: the direct path is available exactly when the writer **is** the owner of the material — the standalone host is (H3), a Python client editing beside another owner is not, and the acknowledgement machinery (O3, D1/D2) exists for that second case and stays.

  **The remote clients are untouched.** This is a *local* data plane, exactly as the buses and taps are: a UDP client goes on using `/buffer_get` and `/buffer_setRange`, which is why S18 comes first — it is what makes that path fast for everyone rather than only for the peers that can map.

  **Cost to the ABI:** the segment layout grows, so `ABI_VERSION` moves and the `release-versioning` rules apply. The `embed` and `--shm` legs are the same change, being one layout.

  **Acceptance:** the standalone host opens a five-minute take with no samples crossing the ring, draws it from the mapped region, writes a stroke with no message sent, and the RT server — a **separate process** — plays the edit on the next block; a peer that maps a freed buffer is refused rather than reading released memory; and `docs/ipc.md` states the buffer region beside the bus one, with the reader-crossing-a-writer rule stated once for both.

  **The five open questions, decided 2026-08-17 before any code.** Two of them had an answer in the existing code that only needed reading; one needed measuring; two are postures this project has already taken elsewhere and are restated rather than re-argued.

  - **The backing: an ordinary enum, matched at every read.** `Buffer` holds `Vec<AtomicU32>`; a shared one holds a mapped region, and the storage becomes a two-variant enum behind the same cell API. **Measured first, as S14's own note demands** (release, an interpolated random read over a 1 M-cell buffer, 64-frame blocks): today's `Vec` **169–278 ns**, the enum matched *per block* **172–281 ns**, matched *per sample* **171–184 ns**, a raw pointer resolved once **170–217 ns**. The four are indistinguishable and the run-to-run noise is an order of magnitude larger than any difference between them — the load from memory is what costs, not the branch, and a buffer's backing never changes so the branch is perfectly predicted. **So no `unsafe` pointer trick**: the plain enum, which is the version that can be read.
  - **The allocation: a region per buffer, outside the segment, with a directory inside it.** The segment is created with `set_len` from the boot options (`segment_size(control_buses, taps, tap_frames)`) and is **fixed for its life**, while a buffer is runtime-sized — a ten-minute stereo take is 230 MB, which is not something an arena can be sized for in advance. So the segment grows a small **directory** (per `bufnum`: generation, frames, channels, sample rate, and the region's name) and each buffer is its own mapped file, named deterministically from the segment's path, the bufnum and the generation. The directory is fixed-size like everything else there — `--max-buffers` rows — so the header keeps its one-shot layout.
  - **The lifetime: the generation counter, which the directory needs anyway.** `/buffer_free` unlinks the region and bumps the generation. A peer still holding the mapping keeps **valid memory** — an unlinked file lives until the last unmap, which is the property this design is built on rather than a trick — and learns it is stale by reading the directory. A later allocation takes a new generation and therefore a new name, so a stale mapping can never be *aliased* onto new material: the worst case is a peer writing into memory nobody reads, which is exactly the failure mode a version counter is for.
  - **The ordering: none is invented.** A peer's stores are not ordered against commands in the ring, and what needs ordering sends `/server_sync`, the barrier that already exists. The only guarantee is the one S14 already states for the audio thread — per-cell atomicity, no ordering between cells — now stated for a second reader.
  - **Who may write: any peer that can map it**, which is the posture the control buses have had since v1 ("the engine reads *these very words*: a client write is live on the next block, no command involved"). A local peer is trusted with the **data** plane; the ring stays the untrusted door, because a *command* is a request and a sample is not.

  **Done 2026-08-17.** `dsp::region::Region` is one buffer's samples as a mapped file; `Buffer` gained a two-variant `Storage` so the same cells are either the server's own memory or a region (the measurement above is why that is a plain enum); the segment grew the **directory** as its tail — ABI **v9**, and no offset moved, since the header had no reserved space for a row count and the count is what remains of the mapped length. A server started with `--shm` puts every buffer it installs into a region and publishes its row; `/buffer_free` retires the row and unlinks the file. `Segment::map_buffer` is the peer's whole door: a path and a buffer number.

  **Three tests, and the third is the one that means anything**: the directory's rules in isolation (a live slot is odd, a retired one maps no more, what was mapped stays readable, a new allocation takes a new generation and therefore a new name); the region round trip (the server's sample read by the peer and the peer's read back by the server, with nothing sent); and a `/buffer_alloc` **through the ring**, mapped by a peer holding only the segment's path.

  **Who inherits it, asked on the day it shipped and answered honestly: nobody yet.** The directory, the region naming and the seqlock read live in the **server crate** (`crates/clausters-server/src/server/ipc.rs`, `crates/clausters-server/src/dsp/region.rs`), and every other reader of a segment is a **hand-written mirror** that pins offsets by hand: `SharedSegment` for the GUI host (`crates/clausters-gui/src/host/shm.rs`) and `ShmClient` for the Python client (`clients/python/clausters/ipc.py`). Both were moved to ABI v9 with this milestone — so neither will attach to a v8 segment or misread a v9 one — and **neither gained the capability**: a client can read the clocks, the buses and the taps of a v9 segment and cannot find the material in it.

  That is the packages-move-together rule half-applied, and the halves are owed to different milestones. **The Python client's landed the same day**: `ShmClient.buffer_info` reads the directory under its generation (a seqlock, like the crate's) and `ShmClient.map_buffer` hands back a `MappedBuffer` whose `samples` is a writable `memoryview` of `f32` — the server's own memory, so a write is what the engine plays next block. Four tests over a hand-built segment, which is the honest shape for a *mirror*: what is under test is that this reader agrees with the layout, and three readers pin that layout by hand. The **host's** is **H6**: mapping what it edits is that milestone's whole subject, and it needs the directory on the mirror it uses when it is *not* linking the server (`--shm` against an external one). The **web client** inherits nothing here by construction: a page cannot map a file, which `docs/ipc.md` states beside the region.

  **What writing the mirror turned up, which is why mirrors get tests**: `ipc.py` declared **1024** control buses against a server that has had **16384** for a long time. Nothing caught it because a client maps the *file's* length and only the documentary `SEGMENT_SIZE` is derived from the constant — so the number was wrong, unused and unnoticed. Corrected, and pinned on both sides now (`crates/clausters-server/tests/ipc.rs` asserts it in Rust, `test_shm_buffers.py` in Python).

  Three mirrors of one layout is the shape this project already lives with for the buses and the taps, and the thing that keeps them honest is that the layout is stated once in `docs/ipc.md` and pinned by a test — the same two guards this region gets.

  **That last paragraph was wrong, and it took a week to prove it.** Prose plus a per-mirror test is not what keeps three readers honest; it is what lets them drift while every test passes. Both failures above are of that kind — a mirror that agreed on the version and not on the size check, a constant that was wrong and unread — and neither could have been caught by a test written *inside* the mirror that held it. The layout moved to `clausters_core::shm` on 2026-08-17 (`docs/decisions.md`, "The segment's layout lives in the shared core"): one definition, a `View` carrying the whole data plane, and a C door (`clausters_core_shm_*`, core ABI v21) for a peer that maps in another language. What each process keeps is getting the memory. What is checked by a compiler now is everything that used to be checked by a comment.

  **The boundary a shared region makes newly necessary, stated here because nothing else can state it.** The segment carries **material, not computation**. A peer may write samples **it already holds** — a pencil stroke's values, a pasted block, a take it loaded — because those are data it is the owner of, exactly as it may write a control bus. Any **operation over samples** (a gain, a fade, a reverse, a render, a resynthesis) stays the server's verbs and is asked for over the ring, however tempting the mapped memory makes it. Without that line this milestone quietly re-opens the question S12 refused — *"an earlier shape had the Python client calling the verbs through the FFI with no server involved, which would have made two performers"* — since a client with write access can compute a fade and store the result, and nothing in the mapping would stop it. What a peer may compute is what it always could: what **drawing** needs (a peak pyramid, an analysis for a picture), which is the rule `gui-widgets` already carries and this does not change.

  **And the mode it is most for is the one with no audio device.** Asked on the day it shipped — *"this can be used with the on-demand NRT server alone, no?"* — and the answer was *by design yes, in the code not yet*: `NrtSession` and the embed both built a **heap** segment, so their buffers stayed in the server's own memory. A session now takes a **path** (`SessionConfig::shm`), and with one its segment is a file and every buffer it installs lives in a region beside it. That is the shape an editor actually wants: the on-demand server renders, applies the edit verbs and holds the material, while an RT server is a separate process that only *plays* it. The embed's own door is **H6's**, since that is where a host boots one.

  **Two things that follow and are worth stating before they surprise someone.** Every pool buffer is shared **when the server has a segment at all** (`--shm` or embedded) rather than on request, so a peer maps by bufnum unconditionally and the directory is never partial; a server with no segment allocates exactly as it does today. And the **browser leg does not get this**: a page cannot map a file, so the wasm engine keeps `/buffer_get` — which is the same split the bulk paths already have, and is why S18 came first.

  **What this milestone got wrong, found 2026-08-17 by reading its own acceptance against what shipped, and corrected rather than quietly closed.** It was marked done with none of its acceptance run: three tests over hand-built segments and no path a person could take from a server to a sound. Three defects had shipped behind that, and they are the same absence:

  - **No example.** Nothing under `examples/` exercised any of it, so the claim "a stroke costs no message" had never been performed. `examples/shm_material.py` is that claim end to end — allocate over the ring, write every sample into the region, play it, edit it in place, play the same buffer again — and `crates/clausters-server/examples/editor_processes.sh` is the arrangement it exists for.
  - **The host mirror refused every real v9 segment.** It was moved to v9 by its *number* and kept a size check that demanded a segment with no directory (`len != expected`), so `clausters-gui --shm` could not open one at all. A version bump cannot catch that; `crates/clausters-gui/tests/material.rs` can, and now does, by putting the two readers side by side under the one feature that links both.
  - **The wasm build had been broken since the day this landed.** `dsp::region` maps files with `libc`, a unix-only dependency, and nothing gated it — so `scripts/check-wasm.sh` and the web package's build failed on it. The material path is gated where it is genuinely a platform thing now, and off Unix a buffer stays the server's own memory exactly as it does with no segment.

  Two rules were added on top of what shipped, both in `docs/decisions.md`: a peer writes **material and announces the span** (`/buffer_touch` → `/buffer_touched`, so a client that cannot map anything — a page above all — still learns a take changed), and a segment has **one owner** (see the entry below).

- ✅ **S20 — A buffer publishes what was written into it** *(opened 2026-08-18 by the user, asking how a live view of a recording can be implemented on every platform, and naming the means: "if needed, even if volatile, files on disk can be used, as multitrack editors do")*. The server half of the GUI host's **H8**, and the two are read together the way S19 and H6 were. It needs S14 (a buffer's cells are written in place, block by block) and S19 (the material is a region beside the segment).

  **The gap, stated exactly.** A peer that edits shared material announces the span it wrote (`/buffer_touch` → `/buffer_touched`, S19), so every other picture of that take can follow. **The engine announces nothing**: `RecordBuf` and `BufWr` fill cells from the audio thread, block after block, and no message is due for that — correctly, since a message per block is the one thing the audio thread must never send. So the one edit that *arrives continuously* is the one nothing can watch, and a take being recorded is drawn only by re-reading it, which is the cost the whole S18/S19 line exists to remove. A scope shows the signal going past; it does not show the take filling up, which is what a multitrack draws while it records.

  **What a writer publishes, and it is a cursor rather than a notification.** The buffer directory's row gains a **write frontier**: the highest frame any writer has filled, stored as one relaxed atomic the audio thread advances once per block (`clausters_core::shm`, beside `frames`/`channels`/`sample_rate_bits`). It is a hint and says so — several writers may share a buffer, and nothing about it is a lock or a promise of what lies before it, exactly as the buffer model has never promised ordering between samples. What it buys is that every reader, in any process, can tell **how far** the material now goes without reading it, and can therefore summarize only what is new. Cost on the audio thread: one store per block per writing UGen, which is the same posture the tap cursor already has.

  **The overview is a file beside the region — volatile, and that is the point.** S19 made a buffer's samples a file the segment names (`region_suffix`); this makes its **peak pyramid** a second one, in the CLPK format the core already writes (`peaks::MultiPyramid::write_cache`, min/max **and** mean square per bucket, a pyramid per channel). It is created with the region, dies with it, and is never a document: an overview is derivable from the material by definition, so losing it costs a rebuild and nothing else. That is what audio editors have always done — Audacity's block files carry a summary of min, max and RMS per 256 samples with a second level over 256 of those, written as the audio is written, and it is what makes a project open without reading its audio.

  **Who fills it, and it is not the audio thread.** The **run loop** does, once per tick: it reads each recording buffer's frontier, summarizes the frames since the last one it summarized (`peaks::Pyramid::update_range`, which exists and is exactly this operation), and writes those buckets into the overview file. At 48 kHz a 20 ms tick is 960 frames, which is four buckets per channel — so the work is bounded by the tick and not by the material, and the audio thread contributes one integer. A reader crossing that write sees some old buckets and some new, which is the same trade the samples themselves take.

  **The wire half, for whoever cannot map anything.** `/buffer_stream periodMs bufnum...` subscribes a client to the same information a mapped peer reads for free: the server acks and then sends, every `periodMs`, one **`/buffer_stream.reply bufnum frontier <buckets blob>`** per listed buffer — the level-0 buckets **since that client's last report**, min/max/mean-square per channel as raw little-endian `f32`. It belongs beside `/bus_stream` and `/bus_tapStream` in `crates/clausters-server/src/osc/server/streams.rs` and takes their whole posture unchanged (one subscription per client, replaced on each call, `periodMs <= 0` cancels, dies with the connection, not schedulable in a bundle). **The unit is the summary and not the samples**, which is the entire argument for the command: one channel at 48 kHz is 187 buckets a second, 2.2 kB/s, against the 192 kB/s the same second costs as audio — a page can watch a stereo take record for the price of a meter.

  **What this is not.** It is not a second way to hear a signal — a scope reads a tap, and that is unchanged. It is not a notification of *editing*: `/buffer_touch` stays what a peer sends about a span it wrote, and this is what the engine leaves behind without sending anything. And it is not a promise that a reader sees every frame — a client that polls slowly sees fewer, coarser reports of the same frontier, and that is correct for a picture whose columns are pixels.

  **Versioning, because a row is a binary boundary.** A field in `BufferRow` changes the segment layout, so `ABI_VERSION` moves to **10** and, by the one-way linkage, the release that carries it bumps SemVer's breaking tier (`release-versioning`, `docs/ipc.md` for what the version changed). `CORE_ABI_VERSION` does not move: nothing new crosses the C ABI here.

  **Done 2026-08-18, and the title lost a clause — the overview file is not part of it.** Staging the work against what H7 had just shipped turned the file from the mechanism into an *optimization of one number*: a peer that maps the region already summarizes a span itself, cheaply and incrementally (`peaks::update_range_from`), so what it lacks is only **how far the material now goes** — which is the frontier and nothing else. The file would save the one pass a take costs when it is opened, and it brings a second writer of derived state with it (who rebuilds it after an edit, and what a reader does with one that disagrees). That is worth taking on its own terms, and it is recorded in "Future directions" rather than smuggled in here.

  **What shipped.** The directory row carries a `frontier` (`clausters_core::shm`, ABI **10**, a row 24 bytes → 32): the highest frame any writer has filled, raised with one relaxed `fetch_max` per block. `RecordBuf` publishes its position and `BufWr` the furthest frame it wrote, both once per block rather than per sample, through a `dsp::buffer::Frontier` sink — a trait, so `dsp` still knows nothing about the IPC layer and an offline render publishes nowhere. A buffer gets its sink where it gets its region (`share_buffer`), so shared material publishes and a server's own memory does not. Republishing a row resets it: the frontier is the *material's*, and the previous tenant's would claim samples this take never got.

  **Both servers publish, and the second one is the one that matters.** A buffer gets its sink where it gets its region — the **owner** when it shares a buffer it allocated (`share_buffer`), and an **attached** server when it maps one (`Segment::map_buffer`). That second case is not symmetry for its own sake: capture belongs to whoever holds the input device (H6), so the process that records is the *player*, which is never the owner of the material. A frontier only the owner could raise would be a frontier nothing ever raised.

  **And the wire half**, for whoever cannot map: `/buffer_stream periodMs bucket bufnum...` → `/buffer_stream.reply bufnum startFrame bucket blob`, in `streams.rs` beside the other two and with their whole posture. The blob is bucket-major, channel-minor: min, max and mean square per bucket per channel — the pyramid's own three statistics in its own energy form. A buffer whose frontier did not move sends nothing, so a still take costs no traffic, and a report carries only what is new.

  **Corrected 2026-08-19: the frontier is the buffer's, and the row is its mirror.** As shipped, `/buffer_stream` read the frontier out of the **segment's** directory row — so a server with no segment reported nothing at all, silently and forever, which is exactly the server most clients that cannot map are talking to (an engine inside a page, a `clausters` booted without `--shm`). The command's whole audience was the one it did not serve. `Buffer` keeps its own `written` counter now and `raise_frontier` mirrors it into the row when there is one; the stream reads the higher of the two, because a **peer** writing into a shared region raises the row and this server's buffer never hears about it. Guarded by `crates/clausters-server/tests/osc.rs::buffer_stream_reports_a_recording_with_no_segment_behind_it`.

  **What a non-Rust client does with it, and why no FFI symbol was added.** The frontier is not exported over the C ABI, on the same reasoning already recorded for the tap rings: nothing outside Rust map-reads material today, and a client that wants to watch a recording has `/buffer_stream`, which carries the summary and needs no mapping at all. `CORE_ABI_VERSION` therefore does not move; the segment's does.

  **The old acceptance, rewritten to what was actually built.** A recorder publishes how far it has written and keeps up with itself (`crates/clausters-server/tests/buffers.rs`); a subscription reports the overview of a written span, reports nothing when nothing moved, and cancels on period 0 (`crates/clausters-server/tests/osc.rs`, over the wire); the audio thread stays allocation-free (`crates/clausters-server/tests/rt_safety.rs`); and the segment's size test carries the new row width, which is the one thing a version number cannot check. **What is left for H8** is the reader: drawing a take as it fills.

  **The acceptance as it was written when the overview file was still in scope:** a recording of five minutes is drawn from a mapped overview while it records, and the host's cost per tick is flat in the take (the number of buckets summarized depends on the tick, not on how long the recording has run); a page subscribed to `/buffer_stream` draws the same picture over WebSocket at a few kilobytes a second, and the two agree bucket for bucket, which is the test that says one summarizer serves both; the audio thread stays allocation-free under `crates/clausters-server/tests/rt_safety.rs`; and an example records a take and draws it filling.

- ✅ **S21 — A segment list reads pool buffers or files, and the RT server plays one from disk** *(opened 2026-10-22 by the user, with the turn it serves: `docs/history/plans/apps.md`, "The turn: an application's takes live on disk", which holds the model and its decisions; done 2026-10-22: a part names a buffer or a PCM WAV span, `TransportDiskIn` plays the list natively and in a page, a locate is at most one block of silence natively, a seam's file opens 2^14 frames ahead against a measured 0.5 ms worst open-and-read, and the ring is 2^17 samples)*. A part of a segment list names either a pool buffer or a span of a file, with the same fades and channel map; the logic is `dsp::stitch`'s, and only where a part's data is differs. A list of pool buffers stays what `/buffer_stitch` makes, read with random access by `BufRd` and its family; a list with file parts is read by a **transport-positioned stream reader**, `DiskIn`'s thread and ring generalized: it follows the transport's position, refills on a locate, and reads the next part's head ahead of each seam, so a seam is a refill at a known frame and never an underrun. A random-access reader refuses a list with a file part, by name. This takes the root entry "A long take is played out of the pool, and `DiskIn` cannot be positioned" (Future directions) whole: the seek, a part that names a file, and the prebuffering. **Measures** the two numbers it has to state: the ring's size and how far ahead of a seam a refill starts. **Acceptance:** a list of quarter-second parts from different files plays from any position without a click, measured at the lead it picks; a locate under a moving playhead lands within a block of silence and never a stale frame; running out of lead is declared silence, counted and reported, and never a click; the audio thread neither allocates nor waits (`crates/clausters-server/tests/rt_safety.rs`); the same reader runs in a page over OPFS (`dsp::disk_web`); the server's memory does not grow with the length of what it plays.
- ✅ **S22 — A file of any length, and a peaks file for it** *(opened 2026-10-22 with `S21`; done 2026-10-22: `dsp::pcm` writes and reads WAV, W64 and RF64 for every reader and writer, natively and in a page; `PeaksOut` writes the CLPK file; a 4.4 GB W64 rendered in 19.6 s reads back identical to a second render with its peaks file; two renders at once write what each writes alone; offline the streams wait for their threads, as scsynth's NRT does. Two things fell short and are named: measuring an hour of stereo floats takes 11.4 s against 0.12 s for one read from the page cache, since the measure runs at the engine's offline pace (about 320 times real time) and reads the file once per channel, while a paced server played on with no sample starved; and a page holds a recording whole, so a take past 4 GB is out of its reach -- `clients/web/PLAN.md`, Found by use, "A recording or a write in a tab is held whole and rewritten at every flush")*. W64 and RF64 are read and written wherever a WAV is (`DiskIn`, `DiskOut`, an NRT render, `/buffer_write`), so a take is limited by its format and not by WAV's 4 GB. A **peaks file** in the CLPK format is written by a **UGen of its own**, beside `DiskOut` and never inside it, since each writes its own data to its own file *(the turn's second decision, taken by the user 2026-10-22)*: a render or a recording that wants its file drawn puts both on the same signal, and a file imported without one is measured once by an NRT render of `DiskIn` into the peaks UGen alone. Where the peaks file is written is the application's to say (the audio editor's temporary folder, a multitrack's tree), so the UGen takes a path like `DiskOut`. No command is added. The base bucket is the format's, 256 frames. **Acceptance:** a W64 file of more than 4 GB written by an NRT render reads back sample for sample, natively and in a page; its peaks file, written by the peaks UGen in the same render, matches a summary of its samples bucket for bucket; measuring an hour of stereo costs about one read of it, runs off the audio thread and leaves an RT server playing without an underrun; two NRT instances render to two files at once and each file and its peaks read back as if rendered alone.
- ✅ **S23 — The server keeps no editing commands** *(opened 2026-10-22 with `S21`; after `X28` and `X29`, once nothing calls them; widened 2026-10-23 by the user with the apps plan's second turn, "an editing application holds its engine", to everything of an application's domain: the segment lists with file parts and their reading, `PeaksOut` as the editor's peaks, and the cue go; `TransportDiskIn` and `/transport_cue` stay if and only if they are useful to the live server (the user, 2026-10-23), measured against the role the transports keep, patterns and sound generated in real time, sample-synchronous playback of files, and OSC and MIDI messages, whose optimizations stay while editing logic goes)*. `/buffer_gain`, `/buffer_reverse`, `/buffer_mix` and `/buffer_render` existed for the audio editor's verbs; in the turn an edit's processing is a def rendered by the NRT server, so they are removed with their builders in both clients, their references in `docs/schemas.md` and their tests, and the copy of a join into a region a peer maps (`share_buffer`) goes with them. `/buffer_stitch`, `/buffer_parts`, `/buffer_peaks`, `/buffer_attach` and `/buffer_touch` stay: they serve pool buffers for every application. **Acceptance:** no client, crate or example names a removed command; the commands that stay are documented as general, with no application's verb in their description; the ABI and SemVer consequences follow the `release-versioning` rules. **Done 2026-10-29:** `/buffer_gain`, `/buffer_reverse`, `/buffer_mix` and `/buffer_render` are gone from the server, both clients (`Buffer.gain`, `fade`, `silence`, `reverse`, `mix`), the references and the tests, and the two examples that used them write with `/buffer_fill` or leave the edit to the audio editor; the copy into a shared region went earlier. **Left, in this order** *(the user confirmed 2026-10-29 that the widened part is done to the letter, the applications' renders moved off what goes)*, against the second turn's model in the apps plan ("the engine knows no file, segment, list or peaks"; the application reads its takes and measures its peaks with the core's functions):
  1. ✅ **The application measures its peaks.** The fold `PeaksOut` does on the audio thread and the pyramid and spectral summary its writer builds become one streaming measurer of the core, fed blocks of samples and writing the CLPK file and the spectral summary beside it. A file opened without peaks is measured by reading it in blocks with the core's PCM reader, and a render's output by folding the blocks the render writes, in the render's own thread; no score carries a `PeaksOut`.
  2. ✅ **A render reads its lists through the application's reader.** A `Render` names the lists it reads and the buffers they are fed into; the render installs those as fed buffers and the application's reader (`take::ReadList`, the one that feeds the live engine) feeds them while it runs; the score no longer stitches files.
  3. ✅ **The server forgets joins, lists and peaks.** *(The audit of 2026-10-29, with the user's verdict: every command made for an application's editing goes, "the server is the module of audio functions, not of editing logic".)* `/buffer_stitch` goes whole, over buffers and over files, with `/buffer_parts`, the list reading behind them (the stream thread's list path, a page's delegated `shapes` job and its bindings) and `Buffer.stitch`/`parts` in both clients; `/buffer_peaks` goes with the summary the server keeps on every buffer (`dsp::summary`), so the GUI host draws a server buffer from samples it reads (mapped, or read back) and summarizes them itself with the core's measure; `PeaksOut` goes *(done 2026-10-29)*; `TransportDiskIn` reads fed buffers only, the application engine's reader. **Stay**, as the live server's: `/buffer_touch` and `/buffer_stream` (a peer's write announced, a recording's progress), the writes and reads of a buffer, and the transport a client plays with (`play`, `stop`, `locate`, `locateSample`, `loop`, `end`, `fade`, `follow`, `group`). *(Done 2026-10-30: the joins, the list reading of the stream thread and of a page, the delegated `shapes` job, `/buffer_peaks`, the summary on every buffer and the overview file beside a shared region are gone; a host that cannot map reads a server buffer back and summarizes it with the core's pyramid, and one that maps summarizes the mapped cells; the audio editor's reader is a `TransportDiskIn` for a fed take beside a `TransportBufRd` for a pool buffer, each silent over the other's, so a host still sounds a take a script drew.)*
  4. ✅ **The cue goes.** `/transport_cue` and its builders leave the wire and both clients; the engine's cue stays as the command an application pre-rolls its reader with. *(Done 2026-10-30: an endpoint's playback carries no cue over the wire, and an OSC score takes none.)*
  5. ✅ **Closed** *(2026-10-30)*: no client, crate or example names what went; the references and the books follow. No binding of the core or of the embed C ABI and no segment layout changed, so neither ABI counter moves; the development version is already a breaking tier past the last tag.
- ⬜ **S24 — A buffer's summary is read like its samples** *(named 2026-10-30 by the user after `S23` closed; **waiting on a decision**: whether it is worth a command at all, and who asks it)*. The server half of a question the GUI host asks: how a host that cannot map the engine's memory — a page, or a native host attached to a remote server over TCP or WebSocket — draws a pool buffer that stands still. Since `S23` it reads every sample back (`/buffer_getRange`, `SpanUse::Whole` in `crates/clausters-gui/src/host/fetch.rs`) and summarizes them itself; a minute of stereo is 23 MB, which a page fills in over a few seconds. A host that maps reads the cells and is not concerned.

  **What it would be.** `/buffer_summary bufnum bucket [start] [frames]` → `/buffer_summary.reply bufnum startFrame bucket blob`, the blob in `/buffer_stream.reply`'s layout (bucket-major, channel-minor, min, max and mean square as `f32`) and computed by the function `/buffer_stream` already calls (`clausters_core::peaks::overview`, through `streams::overview_blob`). **A read, and stateless**, as `/buffer_getRange` is: nothing kept on the buffer, no file beside a region, no join composed, no write counted — the four things that went with `/buffer_peaks` (`docs/decisions.md`, "The server keeps no command an application's editing made"). Each reply reads a bounded number of samples, so an engine in a page, which answers between two audio blocks, is never late; a long span is asked in several requests. The host's answerer of lists already answers a message of that name in that format (`crates/clausters-gui/src/host/disk/answer.rs`), so a host would ask a server buffer and an application's list the same question.

  **What it gives.** A server buffer drawn by a host that cannot map at about a hundredth of the samples' bytes (one bucket of 256 frames carries three values per channel), with samples read back only for the span a zoom shows; `/buffer_stream` already gives exactly that for a buffer being recorded, and this is the same picture of one standing still.

  **What is not settled.**
  - **Whether `plot` uses it.** A plot is not a view of a server buffer: it is a static, measured picture of data the client resolved (a def's render, an envelope, a sequence, a buffer read once with `get_samples`/`getSamples`) and handed to the host whole, and natively shared memory already serves it. Naming the buffer and letting the host ask would change what a plot is in both clients, for a gain only a remote or in-page engine sees.
  - **Whether re-reading on every question matters** for the buffers a script draws, with no cache by construction; natively a pass over a minute of stereo is a few milliseconds.
  - **Which example shows it.** `views/recording` had a finished take built for the route `/buffer_peaks` served (a minute of stereo filled with `/buffer_fill` runs, to be drawn without being downloaded); it was removed with this entry (2026-10-30), and the pair now shows only the four takes recorded live and a peer's edit.

  **Acceptance, if taken:** a page's host draws a minute of stereo from the reply, matching bucket for bucket the summary it computes from the samples; no reply reads more than the bound; the server holds nothing between two questions; the command is documented in `docs/schemas.md` as a read beside `/buffer_getRange`, with its builder in both clients; an example pair shows a buffer standing still drawn from it.

## R track — structural refactoring (the shapes that grew past their module)

Section added 2026-08-02, from a structural review of the workspace crates and
the Python client. Nothing here is a feature: the codebase is healthy (no
`TODO`/`FIXME` anywhere in `src/`, `crates/` or the client; three
non-clippy `#[allow]`s, all justified), and what the review found is the
ordinary sediment of a codebase that grew faster than its file boundaries —
god-objects that accreted five responsibilities, one idiom copied a hundred
times, and one surface declared by hand in three languages.

**The track's invariant: behaviour is preserved, and the existing suites are
the proof.** No wire change, no OSC address added or removed, no client API
signature moved, no golden file touched. A milestone here is finished when the
full suite (plus the def-family matrix and the examples the change touches)
passes with **no test edited** — an edited assertion means the refactor changed
behaviour and is out of scope. Anything that wants a behaviour change is a
different milestone in a different track.

**And cost is part of behaviour.** The suites prove RT-*safety*
(`crates/clausters-server/tests/rt_safety.rs`: no allocation on the audio thread) and sample-identity
(the golden renders), which are not the same claim as *speed*: a refactor can be
allocation-free, bit-exact and still spend more CPU per block, and nothing in
the repo would notice — `crates/clausters-server/examples/bench.rs` and `crates/clausters-server/examples/bench_transport.rs`
measure exactly that, but they are run by hand and CI runs neither. So a
milestone here that touches the audio thread, the command path or the UGen
tables **closes with a before/after `cargo run --release --example bench`**,
both numbers quoted in the commit message. The tolerance is the noise floor of
the machine it ran on (re-run the baseline, don't compare against a number from
another day); anything beyond it is a regression to explain or revert, not to
accept because the tests are green. Two columns carry the claim: "x real time"
for throughput, and the spectral section's **peak block**, which is the one an
average hides. **R11 has since turned this into a CI gate** — a pull request is
benched against its merge base on the same runner, and `scripts/bench-gate.py`
fails it on a regression past the measured threshold — so the before/after is
now done by the machine. Run it by hand anyway while working; the gate is the
backstop, not the instrument.

The exposure is wider than the audio thread, and mostly indirect. R9 is the
only milestone that edits `process_block`/`apply` directly. But R2 and R5 move
code across module boundaries (which can change what the compiler chooses to
inline), R8 restructures the `UGENS` table (which prices synth *instantiation*
— network thread, not RT, but on the latency budget a `/synth_new` answers
within), and R3 rewrites the argument parsing every command flows through.
Those are cheap to check and expensive to discover later, so check them.

**Priority is by bleeding, not by size.** R1 is first because it is the only
item where the current shape actively *produces* drift rather than merely
tolerating it; R2–R4 are next because the OSC front is where every future
command lands.

**R11 and R12 are not refactors — they are this track's guardrails**, and they
are here rather than in a track of their own because they exist for the same
reason the rest of it does: a check that depends on someone remembering is a
check that eventually is not run. R11 makes the cost invariant above
enforceable; R12 makes a release verify anything at all. Neither has to wait
for the refactors, and R12 in particular is worth doing first if a release is
near.

- ✅ **R1 — One declaration per shared-core function** *(done 2026-08-02)* — the
  core's surface was hand-written **three times** (the C ABI, the wasm bindings,
  and 133 lines of ctypes `argtypes` over 155 calls), with only the first
  compiler-checked against core, so a function added to core reached the clients
  only if three edits happened and matched. Both legs are now checked, by
  different instruments, because they owe different things.

  Python owes the C ABI **total coverage**, so its check is a comparison with no
  list to maintain: `ctypes` caches each symbol on the `CDLL` instance as it is
  reached, so after `_configure` runs the instance dictionary *is* the record of
  what the binding declared. It also asserts arity against the Rust signature,
  and catches the quiet case — parameters with no `argtypes` at all, where
  ctypes guesses from the Python values.

  The wasm surface is **legitimately partial**, and the "same set" premise this
  milestone was written with turned out to be wrong: measured, the two bindings
  differ in both directions and most differences are correct (a browser has
  WebSocket, libverovio is not built for wasm, JavaScript has no `u64`, wasm
  frees by `Drop`). Equality would have needed some sixty exemptions, and those
  exemptions were the only interesting content — so the artifact is
  `docs/bindings.md`, a manifest with a verdict per empty cell (`idiom`, `n/a`,
  `gap`), enforced by `tests/bindings.rs`. Divergence stays allowed; divergence
  nobody wrote down does not. Rationale in `docs/decisions.md`.

  Both checks were verified by mutation, since a parity test that passes on its
  first run has proven nothing: dropping a declaration, shortening an argtypes
  list, adding an export to either binding, and leaving a stale row in the
  manifest each fail one of them.

  **What the manifest surfaced, as its first act:** thirteen `gap` rows. Missing
  in wasm — `whitenoise`, `window`, `stats`, the four perceptual scale
  conversions (`hz_to_mel` and friends, which a spectrogram axis wants),
  `patch_compile`, and the clock model's raw `slope`/`intercept`. Missing in the
  C ABI — `spectrum_db`, the three `oscil_*` framing calls, `JsRng.uniform` and
  `JsRng.spawn` (the reproducible-substream primitive), and three `JsRegistry`
  reads. None is urgent and none belongs to this track: they are each either
  small work or a decision to write down, and the manifest is where they now sit
  in the open instead of being invisible.

- ✅ **R2 — Split the OSC front** *(done 2026-08-02)* — `src/osc/server.rs` was
  3217 lines and one `impl` block of 109 methods carrying five separable jobs:
  binding and draining the transports (UDP/TCP/WS/MIDI/IPC), packet and bundle
  dispatch, ~60 command handlers, the streaming subscriptions
  (`BusStream`/`TapStream`), and the async pipelines (NRT/Faust plus the
  `/server_sync` barrier). Pure movement: no signature changed and no test was
  touched, and the only edit the move forced was visibility — the compiler
  named all 62 methods a sibling module calls, so none was promoted on a guess.

  The milestone sketched five files and left the handlers unplaced; they are
  the bulk, so they became `commands/`, one module per resource family — the
  same families the wire names. That keeps the largest file at 500 lines
  instead of folding 1700 lines of handlers into `dispatch`.

- ✅ **R3 — An argument reader, and one voice for failures** *(done 2026-08-02)*
  — every handler used to destructure `msg.args` by hand and write its own
  refusal at each step, which is how 117 `fail` sites came to phrase the same
  four complaints in a dozen ways. `Args` is a cursor whose readers
  (`int`, `index`, `float`, `str`, `long`, `double`, the `opt_*` pair) return
  `Err` instead, handlers return `Answer`, and `OscServer::attempt` is the one
  place a refusal becomes `/fail`. Every command handler now reads through it;
  the count is down to 55, and what is left is not argument shape — the
  dispatch layer, the stream subscriptions and the async pipelines fail about
  other things.

  Beyond the count, two properties the old shape could not have. The failure is
  addressed with **the address the client sent**, so a handler and its dispatch
  arm can no longer drift into disagreeing about what the command is called.
  And `index()` is one read where callers used to write two separate refusals —
  not an integer, and negative — which is exactly the pair that had drifted
  most.

  The bus family is where the duplication was worst: five commands walking the
  same `(base, count, values)` shape with five copies of the same three checks,
  now differing only in the reading. `/synth_get`↔`/synth_getRange` collapsed
  into one body over a shared `control` reader, since resolving a control name
  needs the def and is the one read `Args` has no business doing.

  Two error strings did **not** change, because tests read them: rewording
  `"unknown group"` broke `transport_group_binds_and_unbinds`, and the wording
  went back rather than the test being edited. That is the track's invariant
  doing its job — and the reason to check, before starting, which strings the
  suite is holding.

- ✅ **R4 — Dispatch as a table, checked against the schema** *(done 2026-08-02)*
  — `handle_message` was a ~100-arm `match` on the address string. It is now
  `COMMANDS`, a sorted `&[(&str, Command)]` searched in one binary search, with
  one place that answers `/fail`.

  The table is not faster and was never meant to be; what it buys is that the
  command set became **enumerable**, and `tests/schema.rs` walks it in both
  directions against `docs/schemas.md`: a command the server answers and nobody
  documented, and a command the reference promises and nothing answers. Neither
  was catchable before — the two lived in different files with no way to
  compare them, so the reference could be trusted only as far as somebody's
  memory.

  Two things fell out rather than being aimed at. The dispatcher passes each
  row its own `&'static str`, so the handlers that carry their command's name
  into a later reply (the async buffer jobs) take it from the table instead of
  being told it a second time by their caller, and `/synth_get` distinguishes
  itself from `/synth_getRange` by reading its own address. And `attempt` /
  `attempt_for` disappeared: they existed because a `match` arm could not
  express "run this, and fail with the address that matched", which is now what
  the dispatcher does for every row.

  The parser needed real rules rather than an exclusion list, since the
  reference spells three other things as `/name`: group paths
  (`/mixer/drums`), scsynth's original names cited in the mapping table
  (`/s_new`, `/c_getn` — told apart by their one-letter resource prefix, which
  is exactly what clausters replaced), and the addresses the server *sends*
  (`/done`, the node notifications, `SendReply`'s default `/reply`).

  What it cannot check is whether the *arguments* a page describes are the ones
  the handler reads. That is the deeper drift; it needs types the protocol does
  not have, and this catches the coarse one, which is the one that has actually
  happened.

- ✅ **R5 — Split the translator** *(done 2026-08-02)* —
  `src/osc/translate.rs` was 2747 lines. The milestone named three concerns;
  measured, there were five, and the two it did not name are the ones a reader
  scrolls past looking for the translation proper: what the translator
  *reports* (`def_info`, `query_tree`, `node_info`, `dump_graph` — reply
  arguments, never a `Cmd`) and the `/buffer_*` parsing, which produces
  `NrtJob`s and is not translation at all. So `translate/{graph,midi,queries,
  buffers}.rs` beside a `mod.rs` that keeps the struct, the node and control
  commands, and `translate` itself.

  Pure movement, verified as such: the same 85 `fn` declarations before and
  after, and the whole code stream with whitespace removed differs by exactly
  the module scaffolding plus the seven methods the compiler demanded be
  promoted to `pub(in crate::osc::translate)`. No test touched. The bench came
  back inside the noise floor on every column, which is what the boundary move
  was checked against.

- ✅ **R6 — Split the client's `Server` facade** *(done 2026-08-02)* — the
  mirror image of R2 on the other end of the wire: `defs/server.py` was 958
  lines spanning boot, sending, queries, transport, streams and offline render.
  It is now a package whose `Server` composes `ServerQueries`,
  `ServerStreams` and `ServerTransport`, with the configuration types
  (`ServerOptions`/`ServerInfo`) in `options`. Mixins rather than collaborators
  precisely so no attribute path moves: `server.query_tree(...)` is the same
  call it was, and `from clausters.defs.server import ...` still answers to
  every name it used to.

  **The timeout is now the handle's**, `Server.timeout`, resolved in the two
  methods that actually consume it. The milestone counted 23 copies of
  `timeout: float = 5.0` in this file; there were 31 across the client, because
  `Buffer`, `Bus` and `Node` each carried the literal too and passed it down
  explicitly — which would have made the instance default a lie for half the
  API. All 31 became `None`, so an absent timeout means "the handle's" all the
  way down and `server.timeout = 30` is one assignment instead of an argument
  at every call site.

  The generated API page splits along the same lines (the four new modules are
  listed in `pydoc-markdown.yml`, so nothing left the reference), and
  `clients/web/PLAN.md` carries the shape for the TS port — including the
  timeout, which `defs/server.ts` repeats 26 times.

- ✅ **R7 — The widget's props, declared once** *(done 2026-08-02)* — a prop
  costs three synchronized edits (host, Python builder, TS builder) and nothing
  checked the three agree. It follows R1's decision, and for R1's reason: the
  premise it was written with does not hold.

  **There is no widget registry to generate from.** `host::registry::Registry`
  is bookkeeping over `Map<String, Value>` — it stores whatever arrives and
  knows nothing about which props a `knob` takes. The vocabulary lives in the
  *schema's* two wire passes (`widget::build`, `widget::apply`), one arm per
  kind plus the shared bundles those arms embed. And generation was the wrong
  goal anyway: the explicit signatures **are** the documentation — the Python
  docstring is the widget's user reference and the TS option type is what an
  editor completes.

  So `docs/gui-props.md` is the manifest and
  `clients/python/tests/test_gui_props.py` enforces it: Python read by *calling
  it* (`inspect.signature`, exact), the TS option types and the host's two
  passes read statically. A prop that does not reach all three fails the test
  unless a row says why, with R1's three verdicts. Verified by mutation in six
  directions (a prop dropped from a builder, a prop added to one, a row
  deleted, a row naming the wrong surfaces, a stale row, the host no longer
  reading a prop).

  **What it surfaced, as its first act: 28 rows, 26 pointing the same way.**
  The host implements the timeline chrome (`playhead`, `playhead_loop_*`,
  `sel_*`, `y_*`, `link`), `docs/gui-protocol.md` documents it and the TS
  `TimelineOptions` declares it once for every timeline widget — while the
  Python builders name it widget by widget, so `track` and `timeruler` name
  almost none of it. The other two point the other way and are worse: the TS
  `plot` offers `buffer` and `cache` that the host's plot never reads, so a
  page passing them gets no error and no effect. Closing either is an API
  change, so both are recorded as `gap` rather than fixed here.

- ✅ **R8 — Catalogs split by family** *(done 2026-08-02)* — the two catalogs,
  1971 and 2443 lines, both of them a long list with the families marked only
  by a comment. Navigability only: no symbol moved out of its public path.

  `defs/ugens.py` became `defs/ugens/{graph,osc,filter,pan,io,buf,spectral,
  trig,demand,env}.py`, all 156 names re-exported from `__init__` (plus the
  three private ones other modules already imported). `ugen_input_names` stays
  in `__init__` and not in a family module for a reason worth writing down: it
  maps each server kind to the parameter names of the callable that builds it
  by reading `globals()`, and only there are all the builders in one namespace.
  Proved by comparison against the pre-split module: the same 141 kinds mapped,
  the same signatures and docstrings for all 177 names.

  `src/dsp/registry.rs` became `registry/` with one module per family and
  `FAMILIES`, a slice of slices — an array literal cannot be assembled from
  pieces at compile time, so the catalog is the concatenation, still entirely
  static (no allocation, no lazy init). The families are **contiguous groups in
  the original table order**, which is not a detail: `all()` is what
  `/ugen_query` reports, so a regrouping would have reordered a reply. Verified
  by dumping the catalog before and after — the same 147 kinds, same order,
  same arities and input names.

  `all()` returns an iterator rather than a slice now, which its one caller and
  the uniqueness test adapt to (same assertion, same message). `lookup` runs in
  `compile`, once per def sent — not per `/synth_new` — so instantiation
  latency is untouched; the bench's RT columns confirm nothing else moved.

- ✅ **R9 — The engine's command application, grouped** *(done 2026-08-02)* —
  `Engine::apply` was a ~210-line `match Cmd` of nineteen arms. Done last, as
  the milestone asked, and the shape it takes is decided by the borrow the
  milestone did not account for: `sink` holds three of the engine's fields for
  the whole match, so a `&mut self` helper cannot coexist with it. What is
  separable is the family that touches **only the tree** — twelve of the
  nineteen arms — and that is one free function of two parameters,
  `apply_to_tree(tree, sink, cmd)`, returning the command back when it is not
  one of its own. The other seven each read different engine fields
  (`transport_*`, `sched`, `buffers`, `tap_buses`, `ipc`) and stay where they
  are: 210 lines become 110 plus a homogeneous 111.

  The leftover match names the twelve rather than using a `_`, so the compiler
  still refuses a `Cmd` variant neither half handles — a wildcard there would
  turn that omission into a panic on the audio thread.

  **Measured, paired and alternated on the same machine**, because this is
  audio-thread code: `bench.rs` A/B/A/B reads 1506.5x and 1481.9x before
  against 1486.6x and 1514.2x after (1 synth) — the distributions overlap and
  the spread matches four no-change baseline runs earlier the same day.
  `bench_transport` reads ~6% *faster* after, uniformly, **including its
  0-bundles-per-block rows where `apply` never runs** — which is what says that
  number is code layout in that binary and not the command path. `rt_safety`
  and the golden renders pass untouched.

- ✅ **R10 — The remaining oversized crate files** *(done 2026-08-02)* —
  `clausters-ffi/src/lib.rs` (1543) is now ten domain modules beside the
  `notation.rs` and `ws.rs` that already sat apart: `builtins`, `time`, `scale`,
  `rng`, `sched`, `clocksync`, `registry`, `patch`, `bundle`, `measure`. `lib.rs`
  keeps the crate docs, the ABI version and the re-exports — the C symbols do
  not care which file declares them (`no_mangle` names are flat), but a Rust
  caller does, so `pub use` keeps every `clausters_ffi::…` path. Each test
  travelled with what it tests. Proved by `nm` on the built cdylib: the same 69
  exported symbols, and `tests/bindings.rs` (which reads the whole `src`
  directory) passes untouched.

  `clausters-core/src/bundle.rs` (1287) split at its own seam: `format` (the
  serde types and the one `Error`), `resolve` (the substitution machinery,
  private) and `mod` (the passes, the binding envelope, the tests). The only
  edits movement forced were visibility, and the compiler named each one.

- ✅ **R11 — The performance gate stops being a discipline** *(done
  2026-08-02)* — CI ran fmt, clippy, the test matrix and the GUI/wasm gate, and
  no benchmark at all, so the track's cost invariant shipped only if whoever
  closed a milestone remembered it. It is a job now: on a pull request, the
  merge base and the head are built and benched **in the same job on the same
  runner**, and `scripts/bench-gate.py` compares those two. `[no-bench]` in the
  head commit message steps it aside.

  `crates/clausters-server/examples/bench.rs` grew `--json` — one record per measured row, `{name,
  x_real_time, peak_block, gated}` — beside the human table, which is still the
  default and still prints exactly what it did.

  **The thresholds are measured, not chosen**, and the measuring changed the
  design twice. Three runs of an identical build, compared pairwise: throughput
  moves 0.5% median and 5.6% worst over 210 comparisons — so 10% is a safe
  gate. The peak block does not behave like that: 21% worst for the *aligned*
  peak, 34.6% for the *staggered* one, and **251.8% at one voice**. So the peak
  is gated at 50% and only where it is a measurement — the aligned peak from 32
  voices up. The staggered peak measures whether two chains' hops collided this
  run, which is the stagger working, and is reported instead. Same for the
  Faust rows (the JIT is in the number) and the worker sweep (it reads the
  core count). Rationale and the table in `docs/decisions.md`.

  **Proved in both directions before landing**, since a threshold nobody has
  seen trip is one nobody knows is wired up: three identical runs pass in all
  three pairings, and `#[inline(never)]` on two hot bus accessors turns eleven
  gated rows red at once (`default/1` −10.8%, `sine/ugen/1` −16.2%). The
  pessimization was reverted; the gate's own bootstrap case — a merge base that
  predates `--json` — is handled by the compare step, which says so and passes.

- ⬜ **R12 — A release verifies something.** `release.yml` fires on a `v*` tag
  and chains `build` → `publish-npm` → `publish-pypi` → `github-release`, with
  `needs:` pointing only at each other. There is no `needs:` on CI, no
  `workflow_run` gate, and no `cargo fmt`, `clippy`, `rustdoc` or `cargo test`
  anywhere in the file. A tag on a commit whose CI is red — or that CI never
  ran — publishes to PyPI and npm exactly the same. The workflow already treats
  publishing as one-way where the *contents* are concerned
  (`CLAUSTERS_REQUIRE_COMPLETE: "1"` exists so a piece cannot be silently
  skipped); this closes the other half, where the contents are complete and
  broken.

  Two things to wire, and they are separable:

  - **The gate itself.** A `verify` job at the head of the release, with
    `build` and both `publish-*` jobs `needs:` it. What it runs is the cheap
    question: at minimum the same set CI runs, so the published commit is held
    to the bar every merge is. Rejecting the tag when the commit's CI is not
    green is the alternative, and is weaker — it depends on CI having run at
    all on that exact commit.
  - **The nine configurations nobody automates.** Of the fourteen in
    `.agents/skills/feature-matrix/check.sh`, CI covers five (the two `fmt`s
    and the workspace/gui/default `clippy` runs). The other nine — `clippy`
    under neither def family, `synth` alone, `faust` alone, `clausters-ffi`
    with `verovio`, and all five `rustdoc` builds — are covered by a human
    remembering, which is what the skill exists to remind them of. Running the
    full `check.sh` in the release's `verify` job is the cheapest place to make
    that automatic: once per release rather than once per push, at the moment
    where being wrong is unrecoverable. `check.sh --fast` runs exactly the nine
    if the five are already proven by the same job. The `faust`-linking
    configurations need libfaust, which the release workflow already builds
    and caches through the `.github/actions/libfaust` composite — reuse it,
    don't add a second recipe.

  `check.sh` only ever reads: it is a gate, and it stays one. Nothing about
  wiring it into a workflow introduces a `--fix` pass — clearing a warning is
  deliberate, by hand, on one configuration at a time.

  **Both halves shipped** (2026-08-02): the `verify` job runs `check.sh` in
  full and the two `cargo test` runs, `build` and `publish-npm` each `needs:`
  it, and `CLAUSTERS_FAUST_TARGET` is set on that job alone so the published
  artifacts stay host-tuned. Wiring it turned up the two reds it exists to
  catch, both already on `main` and both fixed alongside: a headless test still
  filtering for a reply address the wire rename had moved, and two TypeDoc
  warnings failing the mdBooks job.

  **⚠ The one thing left, and the only reason this is not ✅: nobody has
  watched the gate stop anything.** Everything about it is verified except the
  behaviour that is its whole purpose. What *is* verified: the `needs:` graph
  (nothing reaches a publish step without `verify`), and the job's contents,
  run by hand on this tree — the fourteen matrix configurations clean and the
  four test configurations green. What is **not**: that a failing `verify`
  actually stops the run.

  This is unproven rather than untried, because the obvious test is unsafe.
  Testing it meant starting a real release; and if `verify` passed when it
  should not have — precisely the misconfiguration under test — the run would
  continue into `publish-npm` and `publish-pypi`, which cannot be taken back.
  **The test's failure mode is the disaster it is testing for**, so it does not
  get run casually, and it did not get run to close this milestone.

  **Half of it is now observed** (2026-08-02): a `workflow_dispatch` trigger
  rehearses the workflow without a tag — `verify` runs exactly as a release
  would, and the four build/publish jobs are guarded by
  `github.event_name == 'push'`, so nothing can reach a registry. The first run
  went green in 6m1s with all four skipped, which retires the weaker of the two
  doubts: the job runs on Actions at all, on the real runner, not just as YAML
  that parses. It says nothing about the stronger one — the dispatch path skips
  the publish jobs whatever `verify` does, so it cannot distinguish a gate from
  its absence.

  How to close the rest safely, when it is worth the time:

  - **On a fork or a scratch repository.** The `npm` and `pypi` environments,
    the `NPM_TOKEN` secret and PyPI's trusted-publisher binding (which names
    this repository *and* this workflow) exist only here, so on a fork a
    publish leg cannot succeed even if it is reached. Push a `v*` tag there on
    a deliberately broken tree — a warning under one def family alone is the
    honest break, since that is the class CI cannot see — and watch `verify`
    go red with everything downstream skipped.
  - **Worth having regardless of the test:** GitHub environment protection
    rules on `pypi` and `npm`. A required reviewer turns any run that reaches a
    publish step into a pause instead of a publication — a second, independent
    stop that does not depend on this workflow being correct.
  - **What does not count as proof:** watching a green `verify` — in a real
    release or in a dispatch rehearsal. That shows the job runs, not that its
    failure stops anything, and those are different claims. A gate only ever
    observed passing is indistinguishable from no gate.

**Not in this track (it is new work, not restructuring).** The review also found
a real parity gap: `defs/ugens.py` exposes 151 builders against
`clients/web/src/defs/ugens.ts`'s 116, and the 40 missing ones are five whole
families (demand, spectral, panning/stereo, convolution, disk I/O) plus the two
SVF filters. Per "the packages move together" that belongs to the web client's
own roadmap, and it does: W6 there already owns it, now carrying the measured
inventory. Recorded next to it, so the same diff does not raise them twice: the
four `gui/guidef.py` helpers that look Python-only are not — `correlation` and
`lissajous` are reachable through the wasm core, and the two file writers have
no page equivalent by design.

## Testing strategy

- **Per-UGen unit tests**: offline render of N blocks, asserts on the signal
  (frequency via zero crossings, RMS, impulse response for filters).
- **Golden files**: NRT mode (M7) renders scenes to WAV and compares with
  tolerance against versioned reference files.
- **RT-safety**: `assert_no_alloc` wraps the callback in test builds; CI runs
  the heaviest graph under that condition.
- **OSC integration**: tests that bring up the server on an ephemeral port and
  talk to it with `rosc` from the test; verifiable by hand with `oscsend` or with sclang
  pointing a `Server` at our port. See the `audio-testing` skill.

## Project skills

- `.agents/skills/realtime-audio` — RT thread rules, lock-free patterns, cpal.
- `.agents/skills/scsynth-osc` — reference for scsynth's OSC protocol and node-tree
  semantics.
- `.agents/skills/ugen-dsp` — the UGens' DSP algorithms (oscillators, filters,
  envelopes) with their formulas.
- `.agents/skills/audio-testing` — how to test audio without ears: NRT, golden files,
  signal asserts, no-alloc.
- `.agents/skills/faust-embedding` — embedding libfaust: C API (box/signal/LLVM),
  factory/instance lifecycle, RT boundaries, JSON→Box API mapping.

## Project notes

- The progress made in each milestone is added to the project notes.
- Closing a milestone always includes, where applicable: the developer
  documentation (`docs/architecture.md`, module docs), the user
  documentation in `docs/` for new features, and an explained example in
  `examples/` if the feature is user-facing — not just the code and the git
  history. The examples are also the manual-test surface: new
  human-audible/visual behavior is checked by running one.

## Future directions (a design that is not a fix)

Opened 2026-08-15 with its first entry. The counterpart of "Found by use"
below: what belongs here is a **design** that has not converged into a
milestone, carrying a checkbox like everything unresolved, and leaving this
list when a milestone absorbs it (the milestone is then the record, and says
where it came from).

An entry that was done without becoming a milestone is ticked and moves whole
to [`docs/history/plans/server.md`](../../docs/history/plans/server.md) ("Future directions: the entries that were done"), so this list
holds what is open.

- ⬜ **Recording, monitoring and playback aligned as one model of the
  transport's latencies** *(asked by the user, 2026-10-23, deciding the
  disk readers' cue)*. Nothing compensates a recording yet: a take recorded
  while the transport plays lands late by the device's round trip, and an
  overdub does not line up with what was heard. It is to be designed whole,
  so the implementation is not one patch behind another. What it has to
  hold together:
  - **The device's latencies**: output and input, as the backend reports
    them (cpal natively, the AudioContext's `outputLatency` and the
    worklet's in a page), and a measured round trip where what is reported
    is not to be trusted. Published where a client reads them
    (`/server_status`), never guessed by a client.
  - **The graph's delay compensation** (`J8`): a path's declared latency is
    already aligned inside the graph; a recording taken after such a path,
    or a monitor sent through one, adds it to the device's.
  - **The disk readers' pre-roll** (the cue, Found by use, "A disk reader
    starts with a click"): what plays is ready before the transport rolls,
    and a recording armed on the same transport starts on the same sample.
  - **Where the shift lands**: a recorded take is placed earlier by the
    total latency on the transport's axis -- a list's part starting at an
    offset, or a region's position -- and its samples are never rewritten
    for it.
  - **Monitoring**: direct (the hardware's) or through the graph, which
    adds the graph's and the device's latency to what is heard while
    recording, and which the person must be able to choose.
  - **Punch in and out, and a loop recording**: the punch points are on the
    transport's axis and the recorded frames are cut there after the shift,
    so a punch is exact on the take whatever the latency.
  - **Several transports started together**: a timetagged bundle starts them
    on one sample; their cues are waited for before it, and their latencies
    are the same device's.
  - **Offline**: an NRT render has no device, so nothing is shifted there,
    and the same score gives the same samples.
  - **A latency that changes** (a plugin reporting a new one, a device
    reopened): a recording in progress keeps the latency it started with.

- ⬜ **A play that runs backwards** *(left by the entry above, 2026-11-08)*. The reader reads backwards; nothing plays a take backwards from where the hand is. What it takes is a playback that maps a backwards reading's frames to the take's (frame `k` of the reading is frame `frames - 1 - k`): the cue, the loop and the end mark of a pass mirrored into the reading's frames, the play cursor drawn at the take's frame rather than at the transport's position, a verb in the audio editor (and the multitrack's tracks, whose `PlacedRead` has no backwards reading yet) and both clients' spelling of it. Scrubbing is the same mapping with a direction that changes while it plays, and a part read backwards inside a forward list -- the editor's reverse as a read rather than a render, which the audio editor's reverse decision allows -- is a field on a part, a document change of its own.

- ⬜ **Per-output and intra-SynthDef latency** — general server compensation
  consumes one declared latency per opaque synth and aligns its bus paths.
  A serial SynthDef chain reports the sum of its intrinsic UGen latencies;
  unequal parallel intrinsic paths inside one def need a per-wire/per-output
  contract and a compiler-side alignment pass. Until that design is adopted,
  expose those paths as separate synths connected by buses. Authored musical
  delays must remain excluded from intrinsic latency.

- ⬜ **Def selector maps over persistent plugin bindings.** Native note/velocity/gate routing and native MIDI program changes are supported; mapping a program to another def, or MPE expression to arbitrary instance parameters, needs an explicit persistent-instance replacement/per-note-parameter policy. Reject those `/midi_map` selectors rather than accepting a map the plugin route ignores.

- ⬜ **`mt.fx` is a declared empty slot, and what an effect *is* as a document
  is unspecified** *(named 2026-09-11, finishing the multitrack's node system)*.
  `mt.clip` and `mt.track` both declare an `fx` slot and both leave it empty,
  which was deliberate: a send to nowhere and an insert of nothing cannot be
  heard and so cannot be checked. **The wiring side needs nothing more** -- the
  slot is a nested-graph slot, so an effect may itself be a GraphDef and a
  compound effect needs no new mechanism.

  What remains missing is the **document**: preset, bypass and tail semantics,
  and how its routing is admitted to the server's automatic domains.

  - **The preset**: what is saved when a document is saved, and what reloading it
    means for an effect whose parameters are a def's and not this project's.
    A leaf is opaque for a reason; an effect's state is the same kind of thing
    and probably the same answer, but it has to be said.
  - **The declared latency and who compensates it**: the server side is
    supplied by M28's `SynthNode::latency()` and J8's general engine plan,
    including isolated-plugin bridge latency. The document still has to name
    its effect routing and its admission to automatic domains; this is not a
    separate compensation algorithm in the master or in each client.
  - **Bypass**: whether it is a state of the chain or of the effect, and whether
    a bypassed effect keeps running (its tail survives, it costs what it cost)
    or stops (it is free, and re-enabling it clicks).
  - **The tail**: what happens to a reverb when the effect is taken out of the
    chain, or the clip under it is deleted, or the transport is located
    somewhere else.

  Until those are answered the slot stays empty, and that is not a gap in the
  node system: the shape of the hole is right, and a wrong answer to any of the
  remaining document questions would be much harder to take back than a missing feature. This entry is what outlives the
  spec the node system was built from.

- ⬜ **The builders could be generated from the catalog instead of contrasted against it** *(named 2026-08-16, after the two contrast tests together caught eleven drifted builders, ten misspelled parameters and one UGen a client never got)*. The server's registry is the source of truth and each client hand-writes a mirror of it; the tests prove the mirrors agree, which is strictly weaker than not having mirrors. Generating `defs/ugens/*.py` and `defs/ugens/*.ts` from the registry would delete the class of bug rather than detect it.

  **The user's condition, stated when the idea was raised** *(2026-08-16)*: it is worth having **if the corroboration happens at compile time**. That points at the shape — not a generator anyone must remember to run, but a checked-in generated file plus a build step that regenerates and **fails on a diff**, the way `docs/bindings.md` is enforced by `tests/bindings.rs`. A generator whose output is not verified by the build is one more thing to forget.

  **What makes it real work rather than a script**: the builders are not tables. They carry the published documentation (docstrings and TSDoc become the API reference), and they carry deliberate ergonomic departures from the wire — the panner family whose builder fills `chan` once per output channel, `env_gen` taking the `Env` first, the static fields that belong behind a `*` or in an options object, the resonance pair given as `rq` or `q`. Every one of those is currently a declared entry in a contrast test's exception list, which is exactly the annotation a generator would need — so the honest sequencing is that the exception lists **are** the specification, and this becomes tractable once they have stopped moving.

- ⬜ **Free routing: a track's output is any bus, and a track can feed another**
  *(named 2026-09-10 in the node system's spec, left open on purpose)*. Group
  buses, folders and a sidechain are one capability: a track's output going
  somewhere other than the master, and an effect reading a signal from elsewhere.
  **What already admits it**: a track's output bus is `external` in its
  `GraphDef` -- the instance that holds it provides it -- so sending a track
  somewhere else is handing it another bus, not a redesign. **What is missing**:
  (a) the document naming a destination as an **identity** (a track, a group
  bus) rather than a bus number, which is an instance's detail; (b) an execution
  order that is no longer the tree's -- a track that feeds another has to run
  first, so a topological order over the routing, with a cycle detected and
  refused by name; the server's M12 auto groups infer order and retain feedback,
  so the document still has to state that stricter cycle policy. (c) Latency
  alignment is supplied by M28/J8 in the server; the document must route its
  parallel paths into the admitted automatic domains rather than duplicate
  compensation in a client.

- ⬜ **Sends: pre- or post-fader is the send's choice, and a meter can read
  either side** *(named 2026-09-10 in the node system's spec)*. **What already
  admits it**: `mt.send.<n>` carries a strip's `post` bus to another bus at a
  gain -- it is what joins a track to the master today -- so an extra post-fader
  send is another instance of that def, and a send is a slot like a clip or an
  effect, with its destination an `external` bus (the entry above). **What is
  missing**: the pre/post choice per send, which is wiring and not a control -- a
  pre-fader send reads the strip's mix bus rather than its `post` -- and the
  destination, which is free routing. A pre-fader **meter** per track is the same
  choice for the meter's slot and lands with it. Sends stay empty until there are
  effects to send to, because a send to nowhere cannot be heard.

- ⬜ **A track wider than stereo, and the downmix between widths** *(named
  2026-09-10 in the node system's spec; `clausters_core::mixer` refuses the
  widths it does not write)*. A strip is written for one and two channels, which
  is what a track declares today. The rules the spec fixed for the rest: N to M
  with N > M sums with the standard table's coefficients (ITU-R BS.775 for 5.1 to
  stereo), written once in `clausters-core`; N to M with N < M and no case above
  takes the first N channels, the rest silent, and the editor says so.

- ⬜ **A silent track could stop costing a cycle, and that is not what mute is**
  *(named 2026-09-10 in the node system's spec)*. Mute is the fader's control --
  it is automated through a bus like any port, it ramps rather than cuts, and it
  keeps an effect's tail and a pre-fader send alive -- so `/node_run 0` on a
  track's group is **not** a mute. It is a separate rule about cost: the
  application could pause a group that is already silent, has no tail to finish
  and no pre-fader send, once the ramp is over. Measured before it is taken, and
  taken without touching the audible rule.

- ⬜ **MIDI 2.0 from outside the server** *(named 2026-09-30, closing
  `crates/clausters-apps/PLAN.md` `X3.11d4`)*. MIDI 2.0 reaches the server
  as instructions of its own -- a transport lane's `ump` list and
  `/midi_ump` -- and neither depends on the operating system. What is not
  there is a **port** a device or another program sends packets through: the
  live input opens a MIDI 1.0 port with `midir`, which speaks no UMP, and no
  Rust crate that is portable does yet. Three ways, each with its cost:
  - **a backend per system behind one interface**, as `cpal` does for audio
    -- the ALSA sequencer's UMP clients on Linux (`snd_seq_set_client_midi_version`,
    in alsa-lib 1.2.10 and later; testable here with `aseqsend -u 2` and
    `aseqdump -u 2`, and a MIDI 1.0 device on the same port arrives
    converted), CoreMIDI's MIDI 2.0 protocol on macOS, Windows MIDI Services
    on Windows -- each needing its own machine to be tested on;
  - **Network MIDI 2.0**, the standard's UDP transport, portable by nature
    and needing no system API;
  - **waiting** for `midir`, or another crate, to carry packets.
  Whichever it is ends at `CmdTranslator::translate_ump`, which the lane and
  `/midi_ump` already reach.

- ⬜ **A GraphDef carries the buffers it plays** *(named 2026-10-01, with the
  user, asking how a GraphDef loads an impulse response)*. A `GraphDefSpec` is
  `name`, `buses`, `members`, `surface` and `defaults`: the private buses are
  the one resource it takes per instance, and a buffer it plays -- a
  waveshaper's transfer function, a reverb's impulse response, a wavetable --
  is the caller's to make before `/graph_new` and to hand over as a port
  value (as `clausters_editing::instance` does with a curve's table,
  `Op::Buffer`). So the def is not whole: every client that instantiates it has
  to know to read, generate or prepare its tables first, in the right order
  (an impulse response is `/buffer_allocRead`, then `/buffer_gen ...
  prepare_partconv` into a second buffer, then the instance). A FaustDef is
  whole in the same situation, since its `waveform` tables travel in its
  source.

  **The shape, as the user put it:** the buffers belong to the **def**, its
  files are kept **in the def's own folder** of the data directory
  (`defs/graphdefs/<name>/`, beside its JSON), the server manages them -- a
  `/def_free` removes them, a re-send replaces them -- and the def and its
  buffers **load together**: `/def_send graph` answers `/done` once its
  buffers are read and prepared on the NRT thread, the barrier a client
  already waits on, and a boot reloads them with the graphdefs. Loading at
  the def keeps `/graph_new` synchronous and atomic, and every instance reads
  one copy of a table nothing writes. The page is no exception: its data
  directory is a directory of the origin private file system, and a page that
  does not persist defs yet is a gap of the port, not of the design.

  It fits what exists -- the async buffer jobs and their `/done`, the data
  directory and its reload order, and the pattern of a reserved range (the
  graph buses, the auto node ids) for buffer numbers the server hands out --
  and leaves four things to decide:
  - **How the bytes reach the server**: inline in `/def_send` (TCP or a
    WebSocket; no impulse response fits a UDP datagram), or a path on the
    server's machine that it copies into the folder (a local client only).
    Probably both.
  - **The vocabulary**: where a buffer's contents come from -- a file, a
    **recipe** the server regenerates (`cheby [...]`, a few numbers and
    deterministic), or a **derivation** of another buffer of the def (the
    kernel `prepare_partconv` makes from the impulse response) -- and how a
    member's `controls` names one, since a string there already means a bus
    (a voice names one by file name, below).
  - **A re-send while instances sound**: a UGen reads its buffer by number on
    every block, so freeing a number silences whatever reads it. A re-send
    makes new buffers under new numbers, and the old ones stay until the last
    instance of the old version ends.
  - **Atomicity**: a buffer that fails to load leaves the def neither stored
    nor replacing the previous one, as `/graph_new` builds nothing by halves.

  **The simplest case it has to cover, as the user put it: a sampler.** One
  SynthDef that plays one of several files, its voices a slot; every
  instance of the def shares the files, and a note makes only the synthesis
  nodes. Three things follow that the shape above does not say yet:
  - **A voice names its sample by file name, never by number** *(decided
    2026-10-01 by the user: "this is usually done by file name")*. The
    numbers are the server's, handed out at load, so a client cannot know
    them. The def keeps a **table, file name to buffer number**, built when
    it loads; a surface port is declared a **buffer** port, and a string sent
    to it (`/graph_newVoice ... sample "kick.wav"`, or a lane note's JSON,
    which already carries strings) is resolved against that table when the
    voice is made, on the network thread -- the node gets a number, as
    today. A string is legal only on a port declared so, which keeps the
    change to the wire a narrow one: every other port value stays a number.
  - **A bank** stays useful beside the names: buffers declared as one ordered
    list and given consecutive numbers, for what reads them by index -- the
    convention `VOsc` morphs across consecutive buffers by.
  - **The table can be asked for**: `/def_query` answers the buffers a def
    holds, their names and the numbers they were given, for a client that
    draws or reads one (a waveform of a sample).
  Which note plays which sample -- key and velocity zones over a bank -- is
  the next question, and the more complex defs start there; it is open here.

  **Related:** "The graphs a note plays in are never freed"
  (`crates/clausters-apps/PLAN.md`, Found by use), decided 2026-10-01:
  generated defs are not persisted (`tmp_`) and are freed by whoever sends
  them, so what is designed here concerns the defs a person authors.

- ⬜ **Third-party plugin formats and native platforms after the Linux reference host** *(named 2026-10-13 with the J track)*. J1-J9 deliver CLAP and VST3 on Linux. LV2, Audio Units and native hosting/editor packaging on macOS and Windows remain possible follow-up work, using the same format interface and client surface. Audio Units are platform-specific. Do not add a second client-side hosting implementation or treat a native editor running on the server as an editor running in a browser. The in-page engine question is recorded in `clients/web/PLAN.md`, **"Native plugins and the in-page engine"**.

- ⬜ **A node's generic editor carries the node's own verbs** *(named
  2026-10-16 at the user's request, after the first by-eye pass of `J12`;
  to be studied and planned)*. Closing the editor of a node leaves the node
  running, which is right for a plugin's editor -- it is a view of an
  instrument that outlives it -- and leaves nothing in the window to stop a
  node that is not one. The editor could carry the node's verbs beside its
  controls: run and pause (`/node_run`), free (`/node_free`), and whatever
  else the node tree admits for it. What has to be settled first: which verbs
  a generic editor offers for which nodes (a plugin, a `Voices` instrument, a
  graph instance, a synth a pattern owns), whether closing the window ever
  frees the node and who decides it (the caller of `edit`, by an option, as
  against the window), how the window shows a node that ended or was freed
  elsewhere -- which the read-back already notices through `/node_end` -- and
  whether the verbs are buttons the core's generator adds to the GuiDef or the
  window's own verbs, in its key table and menu like an application's.

- ⬜ **A buffer position wider than a signal** *(named 2026-10-21, after
  fixing "A long take distorts more the further it plays", Found by use)*.
  A signal between UGens is an `f32` buffer, which counts single frames only
  up to 2^24 (about 5.8 minutes at 48 kHz) and in steps of 2, 4 and 8 after
  that. The transport itself is already 64-bit (`TransportCtx::position` is
  a `u64`); what is narrow is the wire. A buffer index carried on that wire
  repeats and drops frames past that point, which is how a half-hour take
  came to distort. The fix that shipped is `TransportBufRd`, a reader that
  works the phase out in `f64` from the transport's position inside one
  UGen. It covers the transport case and nothing else: **`Phasor` into
  `BufRd` and `Phasor` into `BufWr` keep the limit**, so a buffer read or
  recorded through a phase signal past 2^24 frames still repeats or
  overwrites frames, and `TransportBufRd` is a catalog entry that only
  exists because the wire is narrow. Two ways to remove the limit itself:

  1. **Every signal in `f64`.** The engine's buffers, buses and UGen I/O
     double in width. It removes the limit everywhere at once, and it is a
     rewrite of the engine rather than a change to it: every UGen's
     `process`, the bus pool and the shared-memory segment's layout (an
     `ABI_VERSION` bump), the taps and streams the clients read, the Faust
     and plugin bridges that hand `f32` blocks in and out, and the
     no-allocation and golden tests. It doubles the memory bandwidth of
     every block for a precision almost no signal needs: audio is well
     inside `f32`, and only positions are not.
  2. **Position edges in `f64`, only where a position flows.** The graph
     compiler gives a UGen output a second, double-precision kind of wire,
     declared in the catalog per input and output: `TransportPos` and
     `Phasor` produce it, and the `phase` input of `BufRd` and `BufWr` (and
     any later reader that indexes a buffer) consume it. An ordinary input
     reading a position edge, or a position input reading an ordinary
     signal, converts at the edge, so existing defs and both clients'
     builders are unchanged on the wire. What it touches: the descriptor
     table (a kind per input and output), the def compiler's edge
     allocation and wire buffers (a second pool, of `f64`), the few UGens on
     either end, `/ugen_query` (so the clients see the kind), and Faust and
     plugin nodes not at all, since a position never crosses into them.
     Arithmetic between a position and an offset (`Add`, `Sub`, `Mul` by a
     rate) has to stay on the wide wire for the chain to keep its
     precision, so the binary operators accept a position on either side
     and produce one; that is the part that needs the most care.

  **Recommendation: option 2.** It removes the limit where it exists, for
  every reader and writer and not only the transport's, at a cost confined
  to the compiler and a handful of UGens, and it leaves the audio path's
  width alone. Once it lands, `TransportPos` into `BufRd` is exact again,
  and `TransportBufRd` can be removed in favour of that chain (the audio
  editor's and the multitrack's readers go back to it, and the decision
  note beside the `TransportPos` record in `docs/decisions.md` is updated).
  Option 1 is recorded so it is not proposed again without its cost.

- ⬜ **The transport's commands, applied where the transport is** *(a
  refactoring to evaluate, from the review of `R25`, 2026-11-12)*.
  `Engine::apply` is one `match` over every command, 340 lines, and eight of
  its arms are a transport's alone (run, group, follow, cue, locate, loop,
  end, fade: about 130 lines; with the two lane arms, about 200). They
  could be methods in `server::engine::transport`, called from one line
  each. It is not a move: the garbage sink borrows three of the engine's
  fields for the whole `match`, so each arm's borrows are reordered, and
  what must hold is the order inside an arm (when notes are released, when
  the taken releases run, when garbage leaves). `Schedule` and `ClearSched`
  touch both queues and stay in any case. Against it: the one `match` is
  where every command is seen in one list. It touches the audio thread, so
  it would close with a bench before and after.

## Found by use: the running list of fixes

These are not milestones and they are not future directions. They are what
**using the thing** turns up — an eye pass over an example, a path read while
doing something else, a behavior that is correct and unclear — recorded the day
it is found so it is not rediscovered, and kept here rather than inside whichever
milestone happened to be open at the time.

Every entry is a **checkbox**, so what is open reads as open at a glance and
nothing has to be inferred from where it sits. A fixed one is ticked, with the
record of what was wrong and why the fix is the shape it is, and moves whole to
[`docs/history/plans/server.md`](../../docs/history/plans/server.md) ("Found by use: the entries that were fixed"): this list holds what is
open, and that file is where a search for a fixed one ends.

Anything unresolved lives here or under "Future directions", both **after** the
tracks: never inside the milestone that happened to be open, and never among
finished work, where a pending item reads as done.

- ⬜ **`faust_compiler` failed once in a parallel workspace run** *(found
  2026-10-22, accepting the audio editor)*. `cargo test --workspace` stopped
  on `-p clausters --test faust_compiler`; the suite passed alone five times
  in a row and the whole workspace passed on the next run. The failing output
  was not kept, so neither the case nor the cause is known. What is open is
  the next failure's output: the case, and whether it is load, a libfaust
  call outside `faust::ffi_lock()`, or the compiler cache shared between
  suites.
