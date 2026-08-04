# Local transports: shared memory and embedding (`--shm`, the C ABI)

OSC over UDP makes Clausters controllable from anywhere — but a client on the *same machine* (or in the same process) shouldn't need a network stack to feel like part of one application. Clausters separates the **encoding** from the **transport**: OSC stays the only wire format everywhere, and beside UDP there are two local transports built on one shared **segment**:

| transport | processes | start | client |
|---|---|---|---|
| UDP | any | `clausters` | anything that speaks OSC |
| shared memory | 2, same machine | `clausters --shm <path>` | `clients/python/clausters/ipc.py` (`ShmClient`), or any mmap |
| in-process (embed) | 1 | `cargo build --features embed,realtime` | the cdylib's C ABI, `Clausters` in the Python binding |

All three coexist: a `--shm` server still serves UDP; the embedded server keeps an ephemeral localhost socket as a debug escape hatch. Commands from any transport land in the same queue with the same semantics (timed bundles, `/sched_at`, `/group_sortMode`, …).

## The segment

A single memory region (ABI v4; 722 624 bytes with the default 16 384 control buses, 128 audio buses and 8 audio taps of 16 384 samples) holding, in order:

- **Header**: magic `"CLAU"`, **layout version** — checked on attach, a mismatch refuses to connect (the scsynth plugin-ABI lesson: every binary boundary is versioned) — the device sample rate, the sample clock, the **control-bus count**, the **audio-bus count** and the **tap region shape** (tap count and per-tap ring capacity), so a client maps the whole file and derives every offset from the header alone.
- **Command plane**: two SPSC byte rings (64 KiB each) of OSC packets — client→server commands, server→client replies. Each frame is a `u32` payload length, a `u32` **peer tag**, then the payload padded to 4 bytes. Unlike UDP, a full ring gives **backpressure** (the push fails and you retry) instead of silently dropping packets. Ring contents are untrusted bytes: the server validates exactly as it does UDP datagrams, and garbage resyncs the ring instead of wedging it.
- **Data plane**: the **control buses** as raw `f32`-bit atomics — `--control-buses` of them. These are *the* control buses: the engine's `InCtl` reads these very words, so a client write is live on the next 64-sample block with no command, no round trip, no scheduling. (For sample-accurate changes, keep using a timed `/bus_set` — the data plane trades precision timing for immediacy.) The sample clock in the header is the anchor: a read costs a memory load with zero transport jitter.
- **Audio-bus region**: two words per audio bus — the **bus → tap directory** (which sample ring, if any, is recording that bus) and the **block level** (the peak magnitude of the engine's last block, published for every audio bus). Both are keyed by the bus, which is what makes the bus the only number an API ever names: a reader asks for a bus and finds where its samples land, and a **meter** reads one number per block instead of holding a ring — so metering every bus of a mixer costs no taps at all.
- **Audio taps** (trailing): `--taps` single-channel **sample rings** of `--tap-frames` samples each (a power of two; 0 taps removes the region). Each slot is a cache-line-aligned monotonic **cursor** (total samples ever written) followed by the ring. `/bus_tap tapIndex bus` routes an audio bus into a ring: the audio thread appends that bus's block every block (one `memcpy` + one Release store — RT-safe) and a peer reads the newest window lock-free, cursor-double-checked, each display frame. The audio-rate sibling of the control buses, and what a GUI oscilloscope draws from with zero per-frame messages; clients that cannot map the segment stream the same windows with `/bus_tapStream` (see [`schemas.md`](schemas.md)).

For two processes, put the segment on a memory filesystem (`/dev/shm/...`). The server polls the ring on a 2 ms tick instead of a cross-process semaphore — command latency is bounded by that tick; the data plane has no latency at all. (Semaphore wakeups and Windows named mappings are explicitly future work.)

### Several clients on one segment

One segment carries **several independent clients**, told apart by the peer tag in each frame: on the way in it says who authored the packet, on the way out who the reply is for. The server treats them exactly as it treats two sockets — a `/bus_stream` subscription, a `/server_notify` registration and a reply queue each — which is what a page needs, since its script and its GUI host both push through the one in-page ring. Sharing a tag means sharing a subscription, and `/bus_stream` is *replaced* on each call, so two clients under one tag take the stream from each other.

The tags are the **embedder's to assign**: there is no handshake on the ring and none is needed, because the server only has to tell its clients apart, never name them. A sender that never picks one is peer `0`, which is the single client a segment has always had — so an embedder with one client writes nothing new.

What the tag does *not* change is the SPSC discipline: it says who wrote the **packet**, not who wrote the **ring**. An embedder holding several clients funnels their sends through one producer and demultiplexes the replies by tag — which is exactly what the browser page does, since every send crosses into the worklet through one `MessagePort` and every reply comes back through it.

The doors that carry a tag:

| Surface | Send | Receive |
|---|---|---|
| Rust embed (`clausters::embed`) | `send` (peer 0), `send_as(peer, …)` | `poll_into` (length only), `poll_from` (peer + length) |
| wasm (`clausters-web`) | `WebServer::send(peer, packet)` | `WebServer::poll()` → `[peer u32 LE, …packet]` |
| Python (`clausters.ipc`) | `ShmClient.send(packet, peer=0)` | `poll(peer=0)`, `poll_any()` |
| Embed **C ABI** | `clausters_send` only — peer 0 | `clausters_poll` — length only |

The C ABI is deliberately single-peer: its one consumer is a language client that *is* one client (the Python one), and a second tag there would be surface nobody calls. A C embedder that grows several clients gets `clausters_send_as`/`clausters_poll_to` then, additively.

## The embed C ABI

With `--features embed` the build also produces `libclausters.so` — the **canonical language-agnostic surface**. Per the project's boundary rule, only basic structures cross it: byte pointers in, flat `f32` arrays (pointer + length), integers and error strings out. Check `clausters_abi_version()` first; it moves in lockstep with the segment layout.

```c
uint32_t clausters_abi_version(void);

// The synchronous "scientific" call: render a binary score (the --nrt
// format) and return interleaved float32 samples. NULL on error.
float *clausters_render(const uint8_t *score, size_t len,
                        double sample_rate, uint32_t channels,
                        uint32_t workers, uint64_t *out_frames,
                        uint8_t *err, size_t err_cap);
void clausters_free_samples(float *ptr, uint64_t samples);

// A full live server in this process (audio device + engine + network
// loop); commands are OSC packets delivered by function call.
Clausters *clausters_open(uint32_t workers, uint8_t *err, size_t err_cap);
int32_t clausters_send(Clausters *, const uint8_t *packet, size_t len);
int64_t clausters_poll(Clausters *, uint8_t *buf, size_t cap);
uint64_t clausters_clock(Clausters *);          // data plane, block-accurate
double clausters_sample_rate(Clausters *);
void clausters_ctl_set(Clausters *, uint32_t index, float value);
float clausters_ctl_get(Clausters *, uint32_t index);
void clausters_close(Clausters *);
```

JavaScript reaches the same surface through Node/Deno FFI; in a browser there is no shared memory between processes, so that target waits for a wasm build (where the "segment" becomes a `SharedArrayBuffer` in-process).

## Synchronous calls

Asynchronous replies are the right server model and the wrong interactive ergonomics. The fix lives in the **client**: a blocking facade — send the request, block *this thread* with a timeout until the reply arrives. The server and the audio thread never wait on anything; "synchronous" is purely the caller's view, which is why it composes with every transport:

- over UDP it already exists (`json_client.Client.reply` blocks);
- over the ring, `ShmClient.request()` / `Clausters.request()`;
- fully synchronous with no server at all: `clausters.render(score)` — score bytes in, `array('f')` out, ready for analysis (`numpy.frombuffer` wraps it without copying — the client's choice, never a dependency).

Correlation is by serialization: one request in flight per client, like scsynth clients in practice. A protocol-level correlation token stays deferred until someone actually needs concurrent queries.

## Python binding

`clients/python/clausters/ipc.py`, stdlib only. The transports are named through their module: the `clausters` package re-exports `clausters.ipc` itself, not the two handles, because a script reaches them through `Session.embedded` and `Server.shm` rather than by building one.

```python
from clausters import render
from clausters.ipc import Clausters, ShmClient

c = ShmClient("/dev/shm/clausters")     # two-process
print(c.clock, c.sample_rate)           # data plane reads
c.ctl_set(7, 0.5)                       # live next block, no command
reply = c.request(osc_bytes)            # sync command round trip

with Clausters(workers=2) as s:         # in-process server
    s.send(osc_bytes); print(s.clock)

# sync offline render: samples, frames, events, and the seed it drew
samples, frames, events, seed = render(score_bytes)
```

Demos: `examples/shm_client.py` (attach, watch the clock, fade a synth by writing a control bus in shared memory) and `examples/embed_render.py` (render a score synchronously and write a WAV). Pure-Python caveat: the ring cursors rely on aligned 32-bit accesses being effectively atomic (true on x86-64/aarch64); the Rust sides use real atomics.

The binding loads the cdylibs by this precedence: the `CLAUSTERS_LIB` / `CLAUSTERS_FFI_LIB` env override, then the copies **bundled in the wheel** (`clausters/_libs/`, staged at build time), then the workspace `target/{release,debug}/` of a source checkout. So an installed `pip` wheel is self-contained (no `target/` needed), while a plain checkout still works after `cargo build`. Packaging details and install recipes are in `clients/python/README.md` and [the clients chapter](clients.md#distribution).
