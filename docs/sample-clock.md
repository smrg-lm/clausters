# The sample clock as client timebase (`/clock` and `/sched`)

Clausters extends the scsynth protocol with a second way to time events: scheduling directly on the server's **sample clock** instead of on NTP wall-clock timetags. scsynth has no equivalent — its clients schedule via NTP plus a fixed latency margin and live with the drift.

## Why

The OS clock and the audio device's crystal are two different oscillators; they drift apart by tens of ppm — milliseconds per minute. An NTP-timetagged bundle is converted to a sample position **on arrival**, so every bundle re-anchors against two clocks that disagree: a melody scheduled seconds ahead lands cleanly, but long-running patterns slowly slide relative to the audio, and two clients' grids slide relative to each other.

Inverting the relationship removes the problem at the root: the client asks the server where its sample counter is, models that counter against its own monotonic clock, and addresses every event by **absolute sample number**. The audio clock becomes the master; the OS clock is just the thing the client uses to decide *when to send*.

## Protocol

Two messages, coexisting with the NTP path (both feed the same engine queue — NTP-scheduling clients and sample-scheduling clients can talk to the same server simultaneously):

```text
/clock                    →  /clock.reply  h <samples>  d <sampleRate>
/sched  h <target> b <packet>
```

- **`/clock`** replies with the engine's sample counter (int64 `h`: samples processed since the server started) and the actual device sample rate (double). The counter is the same clock NTP bundles are converted to.
- **`/sched`** schedules a complete OSC packet (the blob) to execute **atomically at the absolute sample `target`** — sample-accurately, the engine splits the processing block at that exact frame, like a timed bundle. The blob may be a single message or a bundle; **all its leaf messages fire at the target** and any timetags inside the blob are ignored: one `/sched` is one instant. The schedulable command set is the same as for timed bundles (`/s_new`, `/n_set`, `/n_free`, `/n_before`, `/n_after`, `/g_new`, `/g_freeAll`, `/g_deepFree`, `/c_set`); anything else replies `/fail` naming the offending message. Past targets execute at the start of the next block, like late NTP bundles. An `i` (int32) target is tolerated for hand-written clients, but real targets outgrow int32 in under 13 hours at 48 kHz — use `h`.

Why a container message instead of a special timetag: the OSC bundle timetag is NTP-formatted *by specification*, so reinterpreting it would silently break every standard client. `/sched` keeps the two timebases explicit and per-event.

`/sched` is itself not schedulable inside a timed bundle (it *is* the scheduling), and neither `/clock` nor `/sched` are valid in NRT scores — score timetags are already converted to exact sample positions offline, so the offline world needs no clock model at all.

## The client model

The reference implementation is `examples/sample_clock.py` (Python, stdlib only). The recipe:

1. **Anchor**: read the local monotonic clock (`t0`), send `/clock`, read it again (`t1`) on the reply. Pair the returned counter with the midpoint `(t0+t1)/2`; the half-width `(t1−t0)/2` bounds the anchor's error.
2. **Model**: fit `sample(t_local) = a + b·t_local` over a sliding window of anchors — least squares with forgetting, in the spirit of JACK's DLL and Ableton Link. Until there are two anchors, use the nominal sample rate as the slope.
3. **Schedule ahead**: convert musical time to sample targets *once* (`step = round(beat_seconds × rate)`) and send each event's `/sched` comfortably before the model says its target will be reached.

Two properties make this robust where it sounds fragile:

- **Query latency does not matter.** The anchor only needs *bounded* uncertainty, not low latency, because nothing fires at query time — events are scheduled ahead. An anchor error shifts the entire grid by a constant; it cannot accumulate.
- **Relative timing is exact by construction.** Two targets N samples apart fire exactly N samples apart, forever — no clock conversion sits between scheduled events. The OS clock's drift only affects how *early* each `/sched` is sent, which the scheduling margin absorbs.

## Caveats

- The counter counts **processed** samples, not heard ones: it runs one device buffer (plus the output latency) ahead of the speakers. For aligning with the outside world (recording overdubs, syncing to video), add the device latency; for pure scheduling it cancels out.
- The counter **pauses on xruns** (no samples are processed). Periodic re-anchoring absorbs this — another reason to keep anchoring in the background rather than anchoring once.
- The counter advances in **device-buffer jumps** (the callback processes its blocks in a burst), so a single anchor sees up to a buffer of quantization. Like query latency this is bounded noise: it widens the slope estimate until the anchor window spans a long baseline (real crystal drift, tens of ppm, needs minutes to resolve), but it never affects *when a scheduled event fires*.
