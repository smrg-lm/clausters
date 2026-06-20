# Local transports: shared memory and embedding (`--shm`, the C ABI)

OSC over UDP makes Clausters controllable from anywhere — but a client on the *same machine* (or in the same process) shouldn't need a network stack to feel like part of one application. M14 separates the **encoding** from the **transport**: OSC stays the only wire format everywhere, and beside UDP there are two local transports built on one shared **segment**:

| transport | processes | start | client |
|---|---|---|---|
| UDP | any | `clausters` | anything that speaks OSC |
| shared memory | 2, same machine | `clausters --shm <path>` | `clients/python/clausters/transport.py` (`ShmClient`), or any mmap |
| in-process (embed) | 1 | `cargo build --features embed,realtime` | the cdylib's C ABI, `Clausters` in the Python binding |

All three coexist: a `--shm` server still serves UDP; the embedded server keeps an ephemeral localhost socket as a debug escape hatch. Commands from any transport land in the same queue with the same semantics (timed bundles, `/sched`, `/g_sortMode`, …).

## The segment

A single memory region (ABI v2; 135 360 bytes with the default 1024 control buses, `fixed_prefix + control_buses × 4` in general) holding, in order:

- **Header**: magic `"CLAU"`, **layout version** — checked on attach, a mismatch refuses to connect (the scsynth plugin-ABI lesson: every binary boundary is versioned) — the device sample rate, the sample clock, and the **control-bus count** (so a client maps the whole file and reads how many buses the server was launched with).
- **Command plane**: two SPSC byte rings (64 KiB each) of length-prefixed OSC packets — client→server commands, server→client replies. Unlike UDP, a full ring gives **backpressure** (the push fails and you retry) instead of silently dropping packets. Ring contents are untrusted bytes: the server validates exactly as it does UDP datagrams, and garbage resyncs the ring instead of wedging it.
- **Data plane** (trailing): the **control buses** as raw `f32`-bit atomics — `--control-buses` of them, a region whose size is the only thing that varies between segments, which is why it sits last (the header and rings keep fixed offsets). These are *the* control buses: the engine's `InCtl` reads these very words, so a client write is live on the next 64-sample block with no command, no round trip, no scheduling. (For sample-accurate changes, keep using a timed `/c_set` — the data plane trades precision timing for immediacy.) The sample clock in the header is the M8 anchor: a read costs a memory load with zero transport jitter.

For two processes, put the segment on a memory filesystem (`/dev/shm/...`). The transport keeps one ring client per segment and the server polls the ring on a 2 ms tick instead of a cross-process semaphore — command latency is bounded by that tick; the data plane has no latency at all. (Semaphore wakeups, multiple ring clients and Windows named mappings are explicitly future work.)

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

`clients/python/clausters/transport.py` (re-exported from the `clausters` package), stdlib only:

```python
from clausters import ShmClient, Clausters, render

c = ShmClient("/dev/shm/clausters")     # two-process
print(c.clock, c.sample_rate)           # data plane reads
c.ctl_set(7, 0.5)                       # live next block, no command
reply = c.request(osc_bytes)            # sync command round trip

with Clausters(workers=2) as s:         # in-process server
    s.send(osc_bytes); print(s.clock)

samples, frames = render(score_bytes)   # sync offline render
```

Demos: `examples/shm_client.py` (attach, watch the clock, fade a synth by writing a control bus in shared memory) and `examples/embed_render.py` (render a score synchronously and write a WAV). Pure-Python caveat: the ring cursors rely on aligned 32-bit accesses being effectively atomic (true on x86-64/aarch64); the Rust sides use real atomics.
