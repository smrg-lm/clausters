# Time, clocks and transports

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## One global transport; the server broadcasts *control*, not audio

Decided with the user: a single global transport, and a conductor's
play/stop/locate drives every client's playhead in lockstep. The server
broadcasts transport *control* (`/transport_play|stop|locate`, pushed to
`/server_notify` clients) and **never schedules audio** — each client rolls its own
playhead on the shared grid. The `/transport_query.reply` carries `playing` and
`position` beside the three tempo fields it started with.

## A moment is a value, and the destination is what stamps it

Sending OSC to an application that is not our server used to be impossible in
any timed sense. The client could *receive* from anywhere (`OscReceiver` +
`OscFunc`), but every path that turned a routine's logical beat into a timetag
was written inside `Server.send_bundle`, wired to that server's own target and
interface. A second destination would have had to copy the arithmetic.

sc3 solves this in the *interface*: `NetAddr.send_bundle(time, ...)` passes a
delta, and each interface resolves it against `main.current_tt._seconds`, with
an NRT subclass overriding `_get_timetag`. That placement gives something real —
nested sub-bundles all resolve against a single captured "now" — and it is why
any `NetAddr` in sclang is timed without knowing what a clock is.

We did not copy it, for two reasons. It rests on `main` being a process-wide
singleton that owns the interface, the main time thread and one physical time
axis in seconds; here `Session` owns the server and the clock, `current_tt` is
thread-local scope rather than configuration, and beats are per clock with
per-clock origins — so an interface cannot know which axis a send belongs to
without asking an ambient global we deliberately do not have. And the argument
that justifies the placement, nested bundles, is not something we encode: the
codec takes a flat list.

So the concepts were split by owner instead. `Moment` — a clock and an exact
beat on it — is the value that answers "what time is it *for this event*", and
it is the routine that carries one, because the routine is what the clock
stamped. A `Destination` owns the target, the carrier and the policy that turns
a moment into wire time. The interfaces keep receiving an absolute instant, so
none of their signatures moved, in either client.

What that leaves in `Server` is exactly what belongs to a server *we control*:
its `latency`, `/sched_at` at an absolute sample, `/server_sync`, and the offline score.
None of those is standard OSC, and none of them is an external application's
business — least of all latency, which is the headroom our audio pipeline needs,
not a fact about someone else's program. An external destination therefore adds
nothing to the timetag; a program that needs to run ahead asks for it as an
explicit delay.

Two things fell out that were not the goal. The clockless case stopped being a
second code path: with no clock a moment is wall-clock now and beats read as
seconds, so `Event().play()` outside a routine goes through the same call as one
inside it, and `send_bundle` with no clock is defined rather than an error.
And `NetAddr` lost its `send_*` methods: it is right as an address — which is
what the name means everywhere outside sclang — and wrong as an emitter, since
emitting needs an interface and a timing policy that an address has no business
holding.

## A transport can pause a piece it cannot seek

Every transport protocol worth surveying — JACK's `jack_position_t` with its BBT
and its slow-sync `Starting` state, Ableton Link, MIDI clock's Song Position
Pointer, VST3's `ProcessContext`, CLAP's `clap_event_transport` — shares one
assumption it never states: **position is an index into samples that already
exists**. `song_pos_beats` means something because something is addressable at
that beat.

A clausters piece generates sound. In the extreme it is one def running
stochastic processes and demand-rate sequences on the server, with nothing to
read and no messages arriving. Such a subtree has no index. Its position *is*
its internal state, and no number summarises it.

So the transport splits what those protocols keep together:

- **Pause and resume are symmetric.** They work identically for samples and for
  a generator, because they are only a freeze of the subtree, the clock and the
  queue. Universal, and cheap — the freeze itself is `NodeTree::set_paused`,
  which already keeps a node in the tree with its state intact.
- **Locate is asymmetric.** Over generated samples it is an index; over a
  generator it does not exist as an operation. `/transport_locate` therefore
  moves the position and never the state of a node, and says so.

This is not a limitation worked around; it is the same law the arrangement layer
already states, arriving at the server. `clausters.form` distinguishes a
*generated* element (random-access: it can be read backwards, sliced, edited)
from a *generator* (forward-only: it can only be evaluated), with **render** as
the change of state between them. A generator becomes locatable by being
rendered — the same answer, one layer down.

It follows that the **locatability flag lives in the client, not the protocol**.
The server cannot know which of its nodes are generators; `form` can, because it
holds the samples. Putting a capability bit on the wire would be the server
promising something it cannot check.

## The two transports were absorbed, not renamed

`/transport_*` already existed as a shared beat grid plus an advisory rolling
state: fields a client read to phase-align, which the server stored and
broadcast but never scheduled audio from. The governing transport needs exactly
those fields plus a group — it is, near enough, a `jack_position_t` without BBT.

The choice was to rename the old one (`/grid_*`, freeing `/transport_*` for the
real transport) or to absorb both into one command family. Absorbing won: one
vocabulary across the three books, `Playhead.follow_transport` unbroken, and no
protocol break for a distinction most clients never make.

The price is one command with two intensities — with no group bound
`/transport_stop` is an advisory, with one bound it freezes the engine — and
that has to be said plainly in the reference rather than left to be discovered.
It is a smaller cost than two parallel families that clients can put into
disagreement with each other.

## A decoded timetag is Unix seconds in every client, and only a reader found out

The three bindings of the shared core agree on what they *export*; nothing makes
them agree on what a value *means* once it has crossed. A timetag argument was
the case where they did not. The Python decoder subtracts the NTP epoch and
hands back Unix seconds, as does the server's own `osc::ntp_to_unix`; the wasm
door built the float straight from `seconds + fractional/2^32` and handed back
NTP seconds — seventy years off, silently, in the one shape a caller cannot
sanity-check by looking at it.

It survived because **no consumer read one**. `/clock_query.reply`'s third
field is the only timetag the server sends as an *argument*, and the sample-clock
tracker models the counter from the first two: the anchor it needs is a local
timestamp, not the server's. The divergence sat in the codec from the day the
codec landed and cost nothing until a wall-clock clock joined a transport, where
mapping the grid's sample origin to Unix time is exactly what the field is for.

Two things follow, and the second is the one worth keeping. The fix is trivial —
the wasm door converts through the core's own function, so there is one
conversion and not two. The lesson is that the parity vectors would not have
caught it: they were **encode** vectors, frozen from the Python encoder and
compared byte for byte, and a decode divergence is invisible to them. Byte parity
on the way out is not value parity on the way in.

So `gen-osc-vectors.py` now writes a second file, `osc-decode-vectors.json`:
packets the Python *encoder* cannot even build — it has no timetag tag — paired
with what the Python *decoder* reads out of them. The generator hand-assembles
the bytes precisely because the reference being frozen is the decoder, not the
encoder. It is a small file and will stay small; what earns its place is that
it covers the direction where a value's *meaning* crosses, which is where two
implementations of one wire drift without any test going red.

## A clock is not a position, and a reader follows the transport rather than carrying one

The transport could say whether a piece was rolling and not where it was. `/transport_locate` moved a number the network thread kept and sent the audio side nothing; the counter the shared segment published — the "transport clock" — counts samples **elapsed** under the transport, so it is monotonic by construction and a locate cannot move it, while the reference called it the time of the piece. Nothing in a graph could read where the piece was at all.

**The two are different quantities and both are needed.** A clock counts what has happened and only goes forward; a position says where you are and moves wherever a locate puts it. The transport scheduler's queue needs the first — "due" only means anything on an axis that cannot jump — and a playhead needs the second. So the position was added *beside* the clock rather than replacing its meaning: `TransportPosition` joins `DeviceSample` and `TransportSample` in `server::clock_axis`, a type for the same reason those are (three `u64`s that mean different things get swapped eventually, and a swapped axis does not fail, it plays audio in the wrong place), and the segment publishes both. `/sched_atTransport` is therefore untouched.

**The position is anchored, not accumulated.** `PositionAnchor` holds the position a locate put the transport at and the transport sample that locate landed on, so a read is one add and a seek is one store — the per-sample path costs nothing, and there is no second counter to keep in step with the first. A loop is the same store: the engine cuts its block at the wrap exactly as it already cuts at a scheduled bundle, so the position advances by exactly one per sample inside every slice.

**That is what lets a reader *follow* the transport.** `TransportPos` is a UGen whose output is the position, and a `BufRd` driven by it seeks when the transport seeks, loops when it loops and holds when it stops — with nothing sent per pass and no position of its own to reconcile. Two consequences worth stating. It is why locating never has to reach into a node, which had read as a limitation ("a locate moves the position, never the state of a node") and is really the other half of the design: a follower has no state to move, and a *generator* has no position because its position is its state. And it is the shape a multitrack needs — many readers, one time — where a reader carrying its own position needs every one of them kept in step by whoever owns the clip list.

The other shape stays available and is not deprecated: `PlayBuf` from its own frame 0, `BufRd` over any phase at all. A one-shot fired from a pattern has no business consulting a transport.

**Two costs, recorded rather than discovered.** The position spends the last of the segment header's reserved space, so the next counter added there moves offsets that out-of-process readers pin by hand. And a signal is `f32`, which counts single frames exactly only to 2²⁴ — about 5.8 minutes at 48 kHz — so `TransportPos` takes an `offset` and subtracts it in `f64` internally: a clip reads its own samples from frame 0 however deep into a long piece it sits, where a graph subtracting afterwards with a `Sub` has already lost what it was trying to keep.

**The offset was not enough, and a reader is one UGen now (2026-10-21).** It keeps a *clip's* phase small, but a take longer than 5.8 minutes, or a window that starts deep into one, has a buffer index past 2^24 however it is subtracted, and the index still crossed from `TransportPos` to `BufRd` as an `f32` signal. Past that point the reader read each frame twice, then four times, then eight, skipping the ones between: a long take distorted more the further it played, which is how it was found (a file pasted at its own end until it ran half an hour). `TransportBufRd` reads at `(position - offset) * rate + start` with the position kept a `u64` and the phase an `f64` until the frame is read, so nothing that varies per sample is ever rounded. Its three operands are still `f32` controls, and that is acceptable for a reason worth stating: they are constant across a reader's life, so their rounding moves where it reads by a fixed amount, which plays every frame, while rounding the ramp repeats and drops them. The audio editor's reader and the multitrack's read with it; `TransportPos` stays for what is not a buffer index (a gate, a fade's position, a curve's table at one sample per block).

## The piece's tempo is a stored integral, and the clock is one segment of it

A beat is not a unit of time. It is a logical coordinate, and the tempo that
turns one into a second can change along a piece — so "when does beat 8
happen?" cannot be answered from the tempo in force now. The two things the
word "tempo" covers are kept apart: the **tempo function** `T(b)` is what a
user writes (a step, a ramp), and the **time map** `M(b) = ∫₀ᵇ db'/T(b')` is
what everything queries.

**We store the integral, not the tempo function.** That is Jaffe's 1985
proposal (*Ensemble Timing in Computer Music*), and the reason is that the map
is what gets asked, constantly, while the tempo function is only what gets
edited — integrating on every query is both expensive and the kind of thing two
callers end up doing differently. `TempoMap` keeps ordered segments with the
seconds cached at each breakpoint, so a query is a binary search plus one
closed-form evaluation: `Δb/T` at a constant tempo, `ln(T₁/T₀)/k` across a ramp
(the analytical route BPMTimeline, WAC 2016, takes). The invariant that makes
all of it work is `T > 0`: it is what makes `M` strictly increasing and
therefore invertible, so it is checked at every edit rather than assumed.

**The consequence that reaches every caller**: a length in beats is not a
duration. `Δbeats` has no length until it is told where it sits, so seconds
come from two *positions* and never from a beat count times a ratio. That is
why `length_to_units`, `Vector.to_event` and the flattening all take an onset,
and why `to_beats(length, tempo)` is the shape to be suspicious of.

**A `TempoClock` did not change; it adopted one segment.** Its
`base_seconds + (beats − base_beats) / tempo` *is* the map's closed form for a
single constant-tempo segment, term for term, so a one-segment map answers with
the identical float — not a close one. What changed is that `set_tempo`
appends a breakpoint instead of overwriting the one anchor it had. The clock
itself never needed the history: it reads its own *now* and moves forward. A
**view** of the piece needs it constantly, and a clock that kept one anchor
would extrapolate its current slope backwards and report that beat 1 happened
at a second it did not. The clock keeps the last segment as an affine cache, so
the real-time path is the same three float operations with no search.

**The map does not reach the server.** `/transport_set` is an origin and one
tempo — a grid several clients phase-align on by reading two numbers — and it
stays that way, because a curve there would be tempo logic on the real-time
side for no gain. The consequence is named rather than papered over: the shared
grid can only express an affine tempo, so joining a server transport *declares
the piece affine*, and a piece whose tempo changes phase-aligns by **sample**
instead (`position_sample`, the axis `/transport_locateSample` already
addresses). Each side speaks its own unit, which is the same rule that keeps a
beat→frame conversion off the wire.

**What is still open** is who owns the map: the clock (execution, lost on save)
or the document (the piece, saved with it). Both are possible today and nothing
decides, so a piece's tempo is not yet persisted.

## A tempo shape is chosen by whether it inverts, not by whether it integrates

The tempo map grew shapes beyond the straight ramp, and the obvious move was to
adopt the whole `Env` family — a piece's tempo and a piece's amplitudes would
then be written with one vocabulary. Most of that family is available and one
part of it is not, and the line between them is not where it looks.

**Integrating is the easy half.** The seconds a segment takes are `∫du/T(u)`,
and every `Env` shape has that in closed form: the straight ramp is a logarithm,
the geometric ramp a difference of reciprocals over a logarithm, the numeric
curvature `(1 - ln((A + B·e^c)/(A + B))/c)/A`, and `sin`, `wel`, `sqr` and `cub`
all integrate too. Each was checked against a 2·10⁶-point numeric integration
before any of it was written.

**Inverting is the half that decides.** `beats_at` — the beat falling on a
second — is not a convenience: it is what `secs2beats` calls, which is what a
running clock calls on every read of `beats()`. The straight and the geometric
ramp invert in closed form. The numeric curvature does not: its integral mixes
`u` and `e^{cu}`, so it is solved by a safeguarded Newton iteration in the core,
which is affordable because a clock's *now* is almost always in the last
segment, and that segment is the step an envelope ends with. `sin` and `wel`
would need the same iteration with none of the same payoff, since the curvature
knob already covers "starts slow" and "starts fast" continuously — and it passes
through linear at `c = 0`, so it is one knob rather than a menu. So the tempo
shapes are **step, linear, exponential and a curvature**, and the rest are
refused by name rather than silently approximated.

The related choice: **the shapes are written over the segment's normalised
position `u`, not over beats.** That is not a formatting preference. It makes
`K = ∫₀¹du/T(u)` independent of how wide the segment is, so a segment `Δb` beats
wide lasts `Δb·K` seconds — and asking for a change that lasts `Δt` *seconds*
inverts to `Δb = Δt/K`, one division, exact for every shape and with no
per-shape special case. Writing the shapes over beats would have made an extent
in seconds a root-find per shape.

### The bindings share a formula, not a libm

The parity tests between the Python client and the wasm one assert **equality**,
and for everything else they can: there is one implementation of each formula
and both clients call it. Tempo shapes are the one place that does not hold.
`exp` and `ln` are not bit-identical between a native libm and the one wasm is
built with, and the shapes evaluate them over a far wider range of arguments
than a straight ramp does. Measured over the shape vectors: **4 of 189 values
differ, by at most 3 ulp**, while the straight-ramp map is still 0 of 16.

Two things follow. `powf` was removed from the core — the geometric ramp is
`exp(u·ln r)` rather than `powf(r, u)`, the same function for `r > 0`, which the
map's `T > 0` invariant guarantees — because `powf` was the worst offender and
costs nothing to avoid. And the shape parity test compares within 4 ulp instead
of exactly, saying so and saying why: the contract is one formula, not one
libm. Equality stays the assertion everywhere it still holds, so a real
divergence would still be caught.

## Who owns a piece's tempo map is the wrong question

*Superseded for the timeline by "A timeline holds its tempo map and plays
itself, and offline time is one logical clock", below: what this entry says of
a map as a value stands; what it says of a `Timeline` knowing nothing of tempo,
and of the saved unit being a named clock, does not.*

The question stood open for a while in this shape: does a piece's tempo belong
to the **clock** (execution — the clock builds it, a save loses it) or to the
**document** (notation — the piece saves it, the clock reads it)? Both worked,
neither was decided, and a piece's tempo was not persisted at all.

It has no answer because it assumes the map is a *part* of something. It is
not. A `TempoMap` is a **value** on the beat axis, the peer of a `Timeline`,
and a `TempoClock` is the *process* that moves over one (`docs/architecture.md`,
"The client's time structures"). A `Timeline` holds `(beat, item)` and knows
nothing of tempo; a `TempoMap` holds tempo and knows nothing of items. Nobody
asks who owns a `Timeline`.

Three things follow, and each of them shipped:

**A value is shared, not copied.** Assigning `clock.map` used to clone, so two
clocks could not read one piece. It adopts now, and the default is unchanged on
purpose — every clock builds its own, so the ordinary case passes nothing. What
a shared map costs is exactly one thing: the driver recomputes every wait from
the map on each pass, so an edit anywhere is *read* correctly with nothing to
invalidate, but a clock asleep on a wait computed before an edit made through
another holder sees it only on waking. `TempoClock.resync` is that call and
`map.version` is what it compares.

**A value is written down.** The map serializes as its **breakpoints** and never
its seconds — those are the cached integral `M(beats)`, and writing a cache out
would let a file assert a second its own tempi do not produce. Loading replays
the breakpoints through the ordinary writers, so a stored map that loads is one
the client could have written, and the door that reads a file is the door that
checks it.

**What an arrangement names is the clock, not the map.** A `Timeline` does not
run by itself: it is played *on* a clock, and several timelines on one clock is
the grouping that means something. So the saved unit is a named clock, and of a
clock exactly two things belong to the piece — its **name** and its **map**.
Its position is transport, its queue is whatever happens to be scheduled, and
its timebase is a choice of the run: pacing against the OS clock or a server's
sample counter says nothing about the music. Polytempo falls out of that and is
the reason it matters — a canon at three tempi is three named clocks, and "the"
tempo of a document could never have expressed it.

What the answer left open is not an owner but a **reference**: a lane naming the
clock it runs on, and a table of clocks in whatever holds a saved piece. Where
that table lives is the open question of what the second document is, since the
arrangement's `Session` holds the source table today and a score has a tempo
too.

**The rule this generalizes to**, which is why it is written down rather than
just fixed: when "who owns X" has no comfortable answer, check whether X is a
value. A value has no owner — it has holders, an identity so a save can name
it, and a serialized form. Only a process has an owner.

## A timeline holds its tempo map and plays itself, and offline time is one logical clock

An audit of every use of `TempoClock` and `TempoMap` found the code treating two
beat axes as one — where an item sits, and how far a clock has run across
passes, `quant`s and restarts — and two tempi as one, the content's and a live
gesture's. The case that worked was the one where the pairs coincide: a bare
clock with routines. Every structure that needed a conversion elsewhere reached
for a scalar or kept a copy of a map it did not own. The frame that resolved
it, decided with the user: **physical time** is seconds and cannot be altered;
**logical time** is beats, metre and notation; a `TempoClock` is a logical
clock that runs; a `TempoMap` is a plan (or a record) of a logical time along
the physical one; the server knows only physical time.

**A `Timeline` is a plan in logical time, so its map is its own.** It holds the
map as data beside its items, and it plays itself on a clock of its own that is
born on its beat 0 — so a clock beat *is* a timeline beat, and the contradiction
a `Playhead` over an external clock had (a map indexed from another zero) cannot
arise. No clock and no playhead are handled for a timeline; a bare clock with
routines stays exactly what it was, and what a timeline needed from a clock — a
seek — was added as `TempoClock.locate`, which moves the logical beat without
moving physical time.

**A timeline contains timelines, played by one engine per tree.** Each child
keeps its units: its beats go to seconds through its own map, from the second
its parent placed it at. The alternative, a running clock per child, costs a
thread per child and propagates every locate and loop clock by clock; one
engine, the root's, keeps nesting, extent, loop, locate and the entry rule in
one place, and it is the same engine a transport's position can later drive.
An item of a child still measures in the child's beats — an event's sustain, an
automation's length, a routine's yields — because the engine hands it a view of
the child's clock that translates to the root's, rather than teaching each item
about nesting. A timeline is stateful, so one instance has one parent (`copy`
makes another), and a routine item is played fresh each pass instead of reset,
since the instance may still be sounding.

**Offline, the system clock is logical, and there is one per run.** Before, each
clock rendered only its own queue and a score's second *was* the emitting
clock's `beats2secs(beat)`: two clocks of a script could not render together, a
clock started at second 4 started at 0, and a locate or a loop had no physical
now to rest on. The fix considered first — a per-clock offset accumulating the
seconds a locate skipped — was a patch for the per-clock render. The model
taken instead is sc3's: one `LogicalTimebase` per offline session, a clock at
tempo 1 that one routine advances by waking whatever is due next across all its
started clocks, in seconds, with every clock's origin on it exactly as a live
clock's is on the monotonic clock. A script then runs the same live and
offline, and a locate is one operation on either time. A clock that is never
started does not play offline, as live; a score never places a bundle before
the run's current second, since a live server plays a past timetag now.

## A timeline on a server transport plans onto it, and the mode is the following

`Playhead.follow_transport` joined two things of different natures: the
transport is **state in physical time** (frozen nodes, a locate and a loop exact
in the engine) and a timeline is **a plan of discrete events in logical time**,
generated by the client ahead of `latency`. Making the playhead react to the
broadcasts left two playheads cancelling each other — the engine's position and
the client's scan — rather than one synchronised piece.

**The mode is a property of the timeline, and the verbs do not change.**
`timeline.transport = server` puts a timeline on that server's transport, and
from there `play`, `pause`, `stop` and `locate` are the transport's own commands,
exactly as the multitrack's playback uses them. What the timeline adds is the
**plan**: every item from the position, stamped on the transport's clock
(`/sched_atTransport`) through the timeline's own map. So a freeze holds the
queue with the piece, a resume carries the frozen sound on with nothing
re-planned, and the walk that writes the plan is the same one the clock mode
wakes — one implementation of nesting, the entry rule and the units, with the
waiting taken out. `timeline.transport_at` places the timeline's beat 0 on the
transport in **seconds**, because the transport's axis is physical and the server
knows no beats.

**A locate is a clear and a re-cue, and the clear had to be scoped.** The
transport clock is monotonic — a locate does not move it — so bundles queued for
the position that was left would sound at it. `/sched_clear` was the only clear
there was and it is global, which would take every other client's score with it,
so it grew an axis: `/sched_clear "transport"` drops the transport queue alone.
The re-cue is stamped `latency` ahead, so what is regenerated is not already
late. A cleared bundle also stopped being reported as a rejected one: the
engine's garbage now distinguishes them, because a clear is what a client asked
for and only an overflow is worth a warning.

**Following is not a second mechanism.** A play, a stop or a locate somebody else
sent arrives as a `/transport_query.reply` broadcast, and the timeline clears and
plans again from where it says — so a conductor drives every follower, and every
follower reads the **one** position the engine holds instead of estimating its
own. That retired `Playhead`, which had no users left: a timeline plays itself on
its own clock or on a transport, and `clausters.form`'s render hands back the
timeline it flattened to.

**What the mode refuses, rather than answering twice.** A `quant` (the start is
the transport's: locate and roll), a `loop` (the wrap is the engine's, and a
timeline's events would have to be re-cued on every one of them), and a
**forward-only item** — a routine or a pattern cannot be planned from a position,
which is the one thing re-cueing from the client requires of its contents. All
three are the client-clock mode's, and a timeline has both modes.

**Several transports on one server stays a future direction** (root `PLAN.md`):
a transport addressed by id reaches the `/transport_*` commands,
`/sched_atTransport`, the `TransportPos` UGen, the shared-memory segment and
`/gui_headClock` at once. The property was named so that it survives: it holds
the server today and one of its transports tomorrow.

## The tempo is the clock's, and a session is the context clocks are made in

*Decided 2026-09-17 with the user, reviewing what was filed as the tempo work's
"scores and render" phase.*

**No verb and no session factory takes a tempo.** `render(tempo=)`,
`Session.nrt/live/embed(tempo=)` and `Timeline.from_pattern(tempo=)` each held a
number that belongs to a clock. None was designed that way; the history shows a
copy chain. The session factories built their clock inside and exposed its one
argument (`5ad93d53`), `from_pattern` built a clock to run a pattern and exposed
the same number (`97f41db4`), and `render` built an offline session and passed
the factory's parameter through (`7cef02ed`) — in both clients. What takes a
tempo now is a clock (`TempoClock`), or a structure that hides its clock
(`Timeline`, whose `tempo` is a constant map). `play` and `render` take an
optional clock, symmetric: one sounds in real time, the other writes audio.
`from_pattern` is gone rather than repaired — a constructor that runs a pattern
to fill a timeline has no reason to exist when an event pattern is an item and a
render. Converting an `OscScore` back into a timeline was proposed and dropped:
a `Timeline` is the score with its generation data, and an `OscScore` is what
rendering one leaves.

**A pattern is the definition of a generator; an `EventPattern` plays.**
`Pattern.play` existed on every pattern, so `play(Pseq([1, 2, 3]))` was taken,
its player failed inside a routine, and `render` reported an unrelated empty
render. `play` now refuses a value pattern by name, `render` generates its
values, and a list pattern (`Pseq`, `Prand`, `Pn`) resolves whether it is an
event pattern when it is built — from whether every element is one, since the
class alone cannot say.

**A clock never changes mode.** `lock_to`, `unlock` and a render that switched a
real-time clock to logical time for its duration all changed a clock's timebase
after the fact, which re-measures every beat already queued against a different
time. A clock's timebase is now fixed when it is made; a clock on a server's
samples is made on `Server.sample_timebase()`, and rendering a clock that is not
on logical time raises. The graceful fall-back to wall-clock time went with it:
there is nothing to fall back to once the clock exists, so a server that does
not answer raises and says why.

**A session is global, per context.** A clock made while a session is active is
made on the session's timebase and kept in it: the server's sample clock for a
live or embedded session, and `LogicalTimebase` for an offline one — the only
time an offline session can have. The context manager is how a session is
switched on the same thread, including inside a running routine, which is what
makes an offline render inside a live session a plain block. One subtlety came out
of the port: in a page everything runs on one thread, so a session merely
*activated* at top level must not take over the wakes of another session's
clock. A context wins over a routine's own session only when it was entered in
that routine; both clients record the routine a context was entered in, and
restore it with the session when the block ends. A timeline's hidden clock
belongs to the session it sounds in, and is made again when it plays in another.

## A server has several transports, each named first on every command

*(Decided with the user, 2026-09-24.)* A server has a table of transports sized
at boot (`--transports`, 8 by default, at most 64), each independent -- its own
rolling state, clock, position, loop, end mark, governed group and scheduling
queue -- because two applications on one server each play and pause their own
nodes, and one transport would give them one playhead. They are several
transports, not several groups under one: groups sharing a transport share its
position, which is the thing that had to be separate.

**The id is the first argument of every transport command, and it is never
optional.** Leaving it out to mean transport 0 was the obvious compatibility
and it does not parse: the server takes an `int32` where it wants a sample
position, because a client sending the smaller type means the number, so a
leading optional integer cannot be told from the argument after it --
`/transport_end 5` is a mark at sample 5 or transport 5's mark cleared, and
`/transport_group 100` a group or a transport. Tightening the sample arguments
to `int64` would have made the leading integer unambiguous for these clients
and silently wrong for a hand-written one. So every command carries it, and an
id past the table fails rather than falling back on 0. The reply that every
transport's pushes share, `/transport_query.reply`, carries the id last, so a
listener tells them apart.

**A node reads the transport that governs it**, and nothing in a def says which:
binding a group tags its slot, and the walk hands a tagged subtree its own
transport's view, so a `TransportPos` in a group bound to transport 1 follows
transport 1. A node under no governed group reads transport 0, which keeps
every graph written before this reading what it read. A group is governed by
one transport at a time, since two would each freeze and thaw it. The clients
keep one surface for it: a `Server`'s transport methods address transport 0,
and `server.transport_at(n)` is the server addressed through transport `n`,
which goes anywhere a server is taken as a transport.

*Revised 2026-10-01:* `server.transport_at(n)` answers a `Transport`, an
object of its own and the same one per id, played as a routine is (`play`,
`pause`, `stop`, `locate`, `loop`, `wait`) and carrying the transport's other
commands by their own names. The view through the server stays as what it
sends through, private. A server seen through a transport was a shape a
GUI playback needed and a script could not use: `play(sequence)` answers the
transport its lane is on, and what it answers has to be played.

The segment's transport counters became a table of 64 rows after the header
(ABI 12), a fixed size so it could stay in the fixed prefix; sized at run time
it would have trailed the rings and moved every offset after it.

**What reads a transport without being governed by it follows it.** An
application's output sits beside its governed group so that it never
freezes -- a meter must fall and a declick must run across the stop -- and
under the rule above it would read transport 0. Rather than give the UGens
that read a transport an id input, which would put a run-time number in a
def, a group can be bound to **follow** a transport (`/transport_follow`): its
nodes read it and nothing freezes them or queues their bundles on it. The
rule stays one rule -- a node reads the transport of the nearest bound group
above it -- and a def still names none.

**The playhead's counter is chosen per view, by id.** `/gui_headClock` had no
id because it said what the numbers a host is handed mean, and one host read
one transport. With several, a host running two applications draws two
timelines, and a window may hold several complex widgets that each play one.
So the command names a window or a widget first -- required, since an integer
may follow the word -- and a widget draws from the counter named on it or on
its nearest ancestor, then its window's, then the host's `--clock`: the same
nearest-ancestor rule the transports themselves follow.

## A stop with a ramp rolls the ramp out, and rests where the readers stopped

*2026-09-24.* A declick on a stop cannot happen after the freeze: on that
sample the governed readers stop producing anything, so there is nothing left
to fade. So a transport with a ramp (`/transport_fade`) stops in two steps --
the governed group goes on running while a level that an output outside it
reads (`TransportFade`) falls to zero, and only then does it freeze.

**During the ramp the transport rolls in full**: its clock, its position and
its queue. Two other readings were weighed and refused. Holding the clock
while the position ran would break the one rule the position rests on, that
it is an anchor onto that clock. Holding the position while the group ran
would have every reader repeat one frame for the length of the ramp -- a DC
offset fading out, which is the thing a declick is there to prevent. So a stop
with a ramp is a stop that lands a ramp's length later, and **the position
rests where the readers stopped reading**: a play continues from there, and
nothing is played twice. The end mark keeps its contract by starting its ramp
that long before the mark, so the pass still ends on it and goes back from
there.

**The length is per transport and none by default.** A stop with no ramp
freezes on its own sample, as it always has, so a transport that asks for
nothing -- the multitrack's, a script's -- behaves exactly as before, and an
application whose output reads the level asks for its own.

## A transport above 0 is taken, not named

A server has a fixed number of transports (`--transports`), and for a while
each application had one by number: 0 the multitrack, 1 the audio
editor, 2 the GUI host's take monitor, 3 the notes editor and every
`play(sequence)`. The numbers were there because nothing allocated. Two of
them, 1 and 2, were the same crate playback held by two endpoints, told apart
by a constant each; and the fourth was shared by every sequence on a server,
so the sequence played last displaced the one before it, and a roll drew a
play cursor that was another sequence's.

Transports are now a space of the ids every client allocates from
(`clausters_core::ids`, `Space::Transports`), sliced by the client's share
like node ids, buses and buffers. Whatever plays on its own takes one and
gives it back: a sequence (one playback per sequence, so two sound together),
the audio editors of a server, the host's monitor. Only **transport 0 stays
addressed by number**: it is what a server's own transport commands address
when no playback is behind them, the grid clients phase on, and a multitrack
plays there.

The cost is that the count is finite and visible, so the default went from 8
to 16 with it: fifteen are above 0, a GUI host sharing the server takes eight
of them, and a client that finds none left is refused with a message naming
`--transports` rather than being handed one in use. Eight had been sized for
four applications with a number each; with a transport per sequence it left a
script beside a host three. A number above 0 picked by hand may be somebody's, so a script takes
one too (`transport_new`), and `transport_at(n)` addresses one it already
knows.

## Time is NTP ticks on every axis (2026-10-24)

A take of hours showed three conversions to 32 bits in one day: a frame cast
to `i32` on the fetch wire, the view event's numbers and the spacing of the
sample dots in `f32`. Each one compiled, passed every test and was found only
by eye, on a take long enough to need the bits. Times in the host and the
applications were bare `f64`, `usize` and `u64`, so nothing told a reader
which of them was a time, and nothing stopped a narrowing cast.

So a time is a type, `clausters_core::time::Time`, and its unit was the
user's choice: **an `i64` of 2^-32 seconds**, the fraction of an OSC timetag.
The alternative was a frame count with a fixed-point fraction, which keeps the
axis in samples and changes less. Ticks were preferred for what they relate:

- **A time on an axis and a timetag are the same arithmetic.** The timetag of
  an axis' origin plus a time is a timetag, exactly, which a time in `f64`
  seconds is not: at the NTP era's four billion seconds an `f64` keeps half a
  microsecond, and the timetag keeps 233 ps.
- **Takes at two rates share one axis.** A position in ticks is the same
  instant whatever the rate of what is drawn at it, where a sample axis needs
  a scale for every take recorded at another rate.
- **Integers add without drift.** A sum or a difference of times is exact,
  and a pixel is computed only from a difference, the time less the view's
  origin, which is small whatever the take's length.

Signed, the range is 68 years either way of the axis' origin, which bounds a
session and not an instant: a wall-clock instant stays a timetag. A sample
index is not a time and stays a whole frame, converted where samples are read
with the rate of what is read. A beat is musical time and stays a beat until a
tempo map turns it into a time. The pass is `crates/clausters-gui/PLAN.md`, "Time is a
type of its own, 64 bits wide".

**The wire stays in frames, and never narrows them.** Two more answers of the
user the same day: the clients' surface keeps speaking frames, and so does the
wire. Ticks on the wire would have needed the rate of every view on both sides
to agree, and a client does not know the rate of a file the host has not read
yet. So a whole frame crosses as an `int64` and a fraction of one, a window's
edge, as a double; the ticks live inside the host and the applications, which
know the rate of each take. What this closed is the narrowing itself: Python
sent every float prop as a 32-bit float and refused an int past 2^31, and a
page sent both narrowed the same way, so a cursor set near the end of a day
long take landed up to 128 frames away.

## Backwards is a reading, not a transport (2026-11-08)

*Taken closing the root plan's "A list reader plays backwards".* Three
places could hold the direction a take is played in: the transport (a
position that runs down), the reader node (a `TransportDiskIn` input that
walks its ring backwards) and the reading (the list the application's
thread renders into the ring). The first changes every transport-positioned
node, the clock axis and what a bundle scheduled on the transport means; the
second changes the one node whose refill, splice, loop and crossfade logic
is the most delicate on the audio thread, and every step of it assumes a
frame that grows.

The third was already there and is the one kept: `TakeRead::Backwards`
renders frame `k` of a reading as frame `frames - 1 - k` of the take, a
chunk read forwards and turned round, so the ring is filled ahead in the
order it is played and the node reads it as it reads any list. A part's
fades, seams, channel map and resampling are the forward reading's,
mirrored, because the frames are read by the same code. The cost is a
mapping where a backwards play meets the take: a pass's cue, loop and end,
and the play cursor, are said in the reading's frames and drawn in the
take's. That mapping belongs to the playback that offers the play, and is
written there when one does.
