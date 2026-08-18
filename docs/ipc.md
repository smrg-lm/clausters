# Local transports: shared memory and embedding (`--shm`, the C ABI)

OSC over UDP makes Clausters controllable from anywhere — but a client on the *same machine* (or in the same process) shouldn't need a network stack to feel like part of one application. Clausters separates the **encoding** from the **transport**: OSC stays the only wire format everywhere, and beside UDP there are two local transports built on one shared **segment**:

| transport | processes | start | client |
|---|---|---|---|
| UDP | any | `clausters` | anything that speaks OSC |
| shared memory | 2, same machine | `clausters --shm <path>` | `clients/python/clausters/ipc.py` (`ShmClient`), or any mmap |
| in-process (embed) | 1 | `cargo build --features embed,realtime` | the cdylib's C ABI, `Clausters` in the Python binding |

All three coexist: a `--shm` server still serves UDP; the embedded server keeps an ephemeral localhost socket as a debug escape hatch. Commands from any transport land in the same queue with the same semantics (timed bundles, `/sched_at`, `/group_sortMode`, …).

## The segment

A single memory region (ABI v9; 820 928 bytes with the default 16 384 control buses, 128 audio buses, 8 audio taps of 16 384 samples and a 4 096-row buffer directory) holding, in order:

- **Header**: magic `"CLAU"`, **layout version** — checked on attach, a mismatch refuses to connect (the scsynth plugin-ABI lesson: every binary boundary is versioned) — the device sample rate, the sample clock, the **control-bus count**, the **audio-bus count** and the **tap region shape** (tap count and per-tap ring capacity), so a client maps the whole file and derives every offset from the header alone.

  It also carries the transport's **two** counters, which answer different questions and are easy to confuse: the **transport clock** (samples *elapsed* under the transport, held while it is stopped — monotonic, so a locate cannot move it) and the **transport position** (where the transport is *in the piece*, in samples of the material — it jumps to wherever `/transport_locate` puts it and wraps at the end of a loop). A playhead reads the position; anything scheduling reads a clock. See [`sample-clock.md`](sample-clock.md).
- **Command plane**: two SPSC byte rings (64 KiB each) of OSC packets — client→server commands, server→client replies. Each frame is a `u32` payload length, a `u32` **peer tag**, then the payload padded to 4 bytes. Unlike UDP, a full ring gives **backpressure** (the push fails and you retry) instead of silently dropping packets. Ring contents are untrusted bytes: the server validates exactly as it does UDP datagrams, and garbage resyncs the ring instead of wedging it.
- **Data plane**: the **control buses** as raw `f32`-bit atomics — `--control-buses` of them. These are *the* control buses: the engine's `InCtl` reads these very words, so a client write is live on the next 64-sample block with no command, no round trip, no scheduling. (For sample-accurate changes, keep using a timed `/bus_set` — the data plane trades precision timing for immediacy.) The sample clock in the header is the anchor: a read costs a memory load with zero transport jitter.
- **Audio-bus region**: two words per audio bus — the **bus → tap directory** (which sample ring, if any, is recording that bus) and the **block level** (the peak magnitude of the engine's last block, published for every audio bus). Both are keyed by the bus, which is what makes the bus the only number an API ever names: a reader asks for a bus and finds where its samples land, and a **meter** reads one number per block instead of holding a ring — so metering every bus of a mixer costs no taps at all.
- **Audio taps** (trailing): `--taps` single-channel **sample rings** of `--tap-frames` samples each (a power of two; 0 taps removes the region). Each slot is a cache-line-aligned monotonic **cursor** (total samples ever written) followed by the ring. `/bus_tap tapIndex bus` routes an audio bus into a ring: the audio thread appends that bus's block every block (one `memcpy` + one Release store — RT-safe) and a peer reads the newest window lock-free, cursor-double-checked, each display frame. The audio-rate sibling of the control buses, and what a GUI oscilloscope draws from with zero per-frame messages; clients that cannot map the segment stream the same windows with `/bus_tapStream` (see [`schemas.md`](schemas.md)).

- **Buffer directory** (trailing, ABI v9): one row per pool buffer — a **generation**, the frame count, the channel count and the sample rate. It says where a buffer's *samples* are, and they are not in the segment: a buffer is sized at run time and can be enormous (a ten-minute stereo take is 230 MB), while the segment is sized once at boot, so each buffer's material is **its own mapped file** beside the segment, named `<segment>.buf<n>.<generation>`. A local peer reads the row, opens the region and has the material — no `/buffer_get`, no blob, no reply — and it writes the same way: the samples it stores are the samples the engine reads on the next block, exactly as a control-bus write is.

  The **generation does three jobs with one number**. It is *odd while the buffer is live* and even when the slot is empty, which is what a peer tests before mapping. It *names the file*, so a freed buffer and its replacement can never share a name and a stale mapping can never be aliased onto new material. And it is a *seqlock*: a writer bumps it before and after the shape, so a reader that sees it move re-reads instead of believing a torn row. Freeing a buffer **unlinks** the region rather than deleting it — every mapping a peer still holds stays valid until it drops it, which is what makes freeing a take safe while somebody is drawing it.

  **It carries material, not computation.** A peer writes samples it already holds — a drawn stroke, a pasted block — the way it writes a control bus. Every *operation* over samples (a gain, a fade, a reverse, a render) is still asked for over the ring and performed by the server, which is the rule the whole system rests on: one place performs audio processing. Mapped memory makes the other thing easy, not correct.

  What the directory does **not** give is ordering: a peer's stores are not sequenced against commands in the ring, and anything that needs them to be sends `/server_sync`. The only guarantee is the one the whole buffer model already makes — per-sample atomicity, no ordering between samples, a reader crossing a writer seeing some old and some new. The row count is what remains of the mapped length rather than a header field, because the header has no reserved space left and a count there would move every offset after it.

### Who owns a segment, and who attaches to one

A segment used to belong to one server for its whole life: `--shm <path>` created the file and truncated it, which was right while the segment was that server's own transport. It indexes the **material** now, so truncating it on the way in would take somebody's take with it — and the process most likely to be restarted is the one holding the audio device. So `--shm` **opens what is there and creates only what is not**, and two roles follow from the one thing a segment cannot have twice:

- **The owner** — the first server on the segment. It claims the **command plane** (a pid in the header, `0` while free), serves the rings, and **publishes the material**: every buffer it installs gets a directory row and a region beside the segment.
- **An attached server** — any later one. It reads the same data plane, **maps** the material the owner published, and serves its clients over its own sockets. It publishes nothing: the directory is one buffer-number space, and two writers of it would hand out the same number twice. Freeing a buffer through it frees its own mapping and leaves the row alone.

The claim exists because the rings are **SPSC**: one pair, one drainer. Two servers popping the inbound ring would each get half the commands, silently. A claim whose pid no longer answers is **stale and is taken over**, which is what makes killing a server a recoverable event — and a server that exits cleanly gives the claim back.

An attached server maps every live row at startup, and a buffer the owner allocates *afterwards* arrives by **`/buffer_attach bufnum`** (see [`schemas.md`](schemas.md)). That is the same line the design draws everywhere: samples never travel, allocation and lifetime always do.

**Who collects a segment nobody serves.** A process that created its segment removes it on the way out, regions and all — but that runs on a clean exit and not on a signal, so a killed editor (a `kill`, a crash, a `timeout` in a script) leaves the file and one region per take behind, and a region is the take's whole size. The claim is what tells a dead segment from a live one, so **creating a segment sweeps the directory first**: a file whose header is ours and whose claim names a pid that no longer exists is removed with its regions. Two rules keep that safe to do to a file this process never created. A claim of **nobody** — `0`, either a segment created a moment ago or one released on a clean exit so it can be adopted — is never swept; only a pid that answered once and does not now. And the path being opened is **never** swept, so adopting a killed owner's material by name works exactly as it did before. Unlinking a name leaves every mapping somebody still holds valid until it is dropped, which is the same property freeing one buffer relies on.

**What attaching does not restore is the routing.** A server that attaches gets the material back and gets no port, no client name and no patch: under JACK or PipeWire the ports and the connections a person made to them live with the *process*. Restarting one is recovery, not a routine — see [`cli.md`](cli.md) for the naming that makes a recovery cheap.

**And the clocks belong to the device.** The header's sample clock and transport counters are written by the process running an audio device; a session with no device never writes them, whether or not it owns the segment. So in an editor's arrangement the owner (the on-demand session) publishes the material and the attached RT server publishes the time, and a playhead reads a counter that means what it says.

### What each layout version changed

`ABI_VERSION` is refused on mismatch rather than negotiated, so what a number means is worth having written down — and **here** rather than beside the constant, where a changelog in a doc comment is a changelog nobody updates.

| v | What changed |
|---|---|
| 5 | the **embed C ABI**, not the segment: `clausters_render` grew a `seed` in pointer form (NULL for a fresh take, a seed to repeat one) and out pointers for the score's event count and the seed it actually used |
| 6 | the **transport clock** beside the sample clock, so a local peer reads what the transport has elapsed with a load. In what was reserved header space, so no offset moved |
| 7 | a **peer tag** on every ring frame, so one segment carries several independent clients. Nothing in the header or the data plane moved — only the framing inside the rings |
| 8 | the **transport position** beside the transport clock: a second quantity, not a redefinition (see above). The last of the reserved header space, so again no offset moved |
| 9 | the **buffer directory** as the segment's tail, and the **control-plane owner** in the word that kept `transport_clock` aligned. A trailing region and a repurposed pad, so nothing before either moved — and the header had no room left for a row count, which is why the count is what remains of the mapped length |

### One definition, and the readers that follow it

The layout above is **`clausters-core`'s** (`clausters_core::shm`), and every process that touches a segment reads it from there: the server that writes it, the GUI host, and — through the C ABI (`clausters_core_shm_*`, see [`bindings.md`](bindings.md)) — a `ctypes` or N-API client. What each one still does for itself is *getting* the memory: `mmap` of a file, a heap allocation, Python's `mmap`. That is the genuinely platform-shaped part, and it is the only part.

It was not always so, and the reason it is now is worth keeping. Three readers used to mirror the `#[repr(C)]` by hand with the version counter as the only tie between them, and that failed twice in one week in two different ways: a mirror that agreed on the version number and not on the size check refused every valid segment, and another declared 1024 control buses against a server that had had 16 384 for months — wrong, unused, and invisible to every test. A number cannot check a layout.

So a foreign reader asks for the **shape** (every count and every byte offset, in one call) and for the things that are logic rather than arithmetic: the directory's seqlock, the ring's framing, the name a region file carries. What is left in each binding is one struct declaration.

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
