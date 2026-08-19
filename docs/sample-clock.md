# The sample clock as client timebase (`/clock_query` and `/sched_at`)

Clausters extends the scsynth protocol with a second way to time events: scheduling directly on the server's **sample clock** instead of on NTP wall-clock timetags. scsynth has no equivalent — its clients schedule via NTP plus a fixed latency margin and live with the drift.

## Why

The OS clock and the audio device's crystal are two different oscillators; they drift apart by tens of ppm — milliseconds per minute. An NTP-timetagged bundle is converted to a sample position **on arrival**, so every bundle re-anchors against two clocks that disagree: a melody scheduled seconds ahead lands cleanly, but long-running patterns slowly slide relative to the audio, and two clients' grids slide relative to each other.

Inverting the relationship removes the problem at the root: the client asks the server where its sample counter is, models that counter against its own monotonic clock, and addresses every event by **absolute sample number**. The audio clock becomes the master; the OS clock is just the thing the client uses to decide *when to send*.

## Protocol

Two messages, coexisting with the NTP path (both feed the same engine queue — NTP-scheduling clients and sample-scheduling clients can talk to the same server simultaneously):

```text
/clock_query                    →  /clock_query.reply  h <samples>  d <sampleRate>  t <oscTime>
/sched_at  h <target> b <packet>
```

- **`/clock_query`** replies with the engine's sample counter (int64 `h`: samples processed since the server started), the actual device sample rate (double `d`) and the server's OSC/NTP time captured together with the counter (timetag `t`). The counter is the same clock NTP bundles are converted to. The `(oscTime, samples)` pair is an **anchor**: it ties the server's sample axis to OSC time at one instant, so a client can convert a logical OSC time `T` to a sample with `sample ≈ samples + (T − oscTime)·rate` and place events on the server's clock without first modelling the counter against its own local clock — the basis for one server acting as a **master clock** for several clients sharing one drift-free sample axis. Clients that only need the older two-field form ignore the trailing timetag.
- **`/sched_at`** schedules a complete OSC packet (the blob) to execute **atomically at the absolute sample `target`** — sample-accurately, the engine splits the processing block at that exact frame, like a timed bundle. The blob may be a single message or a bundle; **all its leaf messages fire at the target** and any timetags inside the blob are ignored: one `/sched_at` is one instant. The schedulable command set is the same as for timed bundles (`/synth_new`, `/node_set`, `/node_free`, `/node_before`, `/node_after`, `/group_new`, `/group_freeAll`, `/group_deepFree`, `/bus_set`); anything else replies `/fail` naming the offending message. Past targets execute at the start of the next block, like late NTP bundles. An `i` (int32) target is tolerated for hand-written clients, but real targets outgrow int32 in under 13 hours at 48 kHz — use `h`.

Why a container message instead of a special timetag: the OSC bundle timetag is NTP-formatted *by specification*, so reinterpreting it would silently break every standard client. `/sched_at` keeps the two timebases explicit and per-event.

`/sched_at` is itself not schedulable inside a timed bundle (it *is* the scheduling), and neither `/clock_query` nor `/sched_at` are valid in NRT scores — score timetags are already converted to exact sample positions offline, so the offline world needs no clock model at all.

## The client model

The reference implementation is `examples/sample_clock.py` (Python, stdlib only). The recipe:

1. **Anchor**: read the local monotonic clock (`t0`), send `/clock_query`, read it again (`t1`) on the reply. Pair the returned counter with the midpoint `(t0+t1)/2`; the half-width `(t1−t0)/2` bounds the anchor's error.
2. **Model**: fit `sample(t_local) = a + b·t_local` over a sliding window of anchors — least squares with forgetting, in the spirit of JACK's DLL and Ableton Link. Until there are two anchors, use the nominal sample rate as the slope.
3. **Schedule ahead**: convert musical time to sample targets *once* (`step = round(beat_seconds × rate)`) and send each event's `/sched_at` comfortably before the model says its target will be reached.

Two properties make this robust where it sounds fragile:

- **Query latency does not matter.** The anchor only needs *bounded* uncertainty, not low latency, because nothing fires at query time — events are scheduled ahead. An anchor error shifts the entire grid by a constant; it cannot accumulate.
- **Relative timing is exact by construction.** Two targets N samples apart fire exactly N samples apart, forever — no clock conversion sits between scheduled events. The OS clock's drift only affects how *early* each `/sched_at` is sent, which the scheduling margin absorbs.

## A shared transport (phase alignment)

The sample clock gives every client a common, drift-free **sample axis**, so two clients locked to one server schedule on the same timeline. It does not, by itself, put them in **phase**: each client's routine still starts whenever it starts, at an arbitrary point on that axis. Aligning several clients on the *same* beat needs a shared **beat grid** — an origin and a tempo everyone agrees on.

`/transport_set` is that grid, hosted on the server so any client can join it. It also carries a DAW-style **rolling state** — whether it is playing and the song position:

```text
/transport_query                              →  /transport_query.reply  h <originSample>  d <tempo>  i <defined>  i <playing>  d <position>  i <group>  h <transportSample>  h <positionSample>  h <loopStart>  h <loopEnd>
/transport_set  h <originSample> d <tempo>    →  /done  "/transport_set"          (set the grid, stopped at 0)
/transport_play  [d <position>]           →  /done  "/transport_play"     (start rolling)
/transport_stop                           →  /done  "/transport_stop"
/transport_locate  d <position>           →  /done  "/transport_locate"   (set the song position, in beats)
/transport_locateSample  h <sample>       →  /done  "/transport_locateSample"  (the same, in frames of the piece)
/transport_loop  [h <start> h <end>]      →  /done  "/transport_loop"     (wrap inside this span; no arguments = off)
```

- `/transport_query` **reads** the grid; `defined` is 0 until a client has set one, and `group` is `-1` until one is bound.
- `/transport_set` **sets** the grid (last writer wins), stopped at position 0. Beat `b` maps to sample `originSample + b·rate/tempo`, so a client joins by reading the grid and quantizing its routine's start onto the next beat boundary. Because the grid lives on the sample axis, the alignment is sample-exact for clients locked to the master.
- `/transport_play`/`/transport_stop`/`/transport_locate` drive the rolling state (a conductor): play from a song position, stop, or seek. **Only the beat spellings need the grid** — `/transport_locate`, and `/transport_play` given a position, refuse until one is defined; a bare play, a stop, a `/transport_locateSample` and a `/transport_loop` need none, because they are all in samples and an editor has no tempo to declare.
- **A clock is not a position, and the transport publishes both.** `transportSample` counts samples *elapsed* under the transport: it holds while stopped and never goes backwards, which is exactly what the scheduler's transport queue needs, since "due" only means anything on an axis that cannot jump. `positionSample` is where the transport **is in the piece** — it jumps wherever a locate puts it and wraps at the end of a `/transport_loop` span. A playhead draws the position; anything scheduling reads the clock. Both are in the shared-memory segment, and a graph reads the position through the `TransportPos` UGen, which is how a buffer reader follows the transport instead of keeping a position of its own.
- **`/transport_locateSample` addresses that position in frames**, where `/transport_locate` addresses it in beats. Both spellings stay in step; which one to use is which one your samples is measured in — an editor's is frames, a sequencer's is beats, and converting between them on the client is how a rounding error gets into a seek.

Every change is **pushed** to every `/server_notify` client as a `/transport_query.reply`, so a client with a responder on it re-aligns or rolls its playhead live — no polling. With no group bound the server stores, serves and broadcasts the transport but **schedules no audio from it** — it is shared control, not a sequencer, and each client rolls its own playhead on the shared grid. Binding a group with `/transport_group` is what changes that: the engine then freezes that subtree and the transport clock on a stop, and `transportSample` is the piece's own time. See [`schemas.md`](schemas.md). The grid is in-memory (the sample counter resets on restart, so an origin only means anything within one run) and ownership is last-writer-wins.

## Caveats

- The counter counts **processed** samples, not heard ones: it runs one device buffer (plus the output latency) ahead of the speakers. For aligning with the outside world (recording overdubs, syncing to video), add the device latency; for pure scheduling it cancels out.
- The counter **pauses on xruns** (no samples are processed). Periodic re-anchoring absorbs this — another reason to keep anchoring in the background rather than anchoring once.
- The counter advances in **device-buffer jumps** (the callback processes its blocks in a burst), so a single anchor sees up to a buffer of quantization. Like query latency this is bounded noise: it widens the slope estimate until the anchor window spans a long baseline (real crystal drift, tens of ppm, needs minutes to resolve), but it never affects *when a scheduled event fires*.
