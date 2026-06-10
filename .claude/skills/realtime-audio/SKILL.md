---
name: realtime-audio
description: Rules and patterns for writing real-time audio code in Rust with cpal — what is forbidden on the audio thread, lock-free communication with rtrb/basedrop, and callback handling. Read before touching any code that runs inside the audio callback.
---

# Real-time audio in Rust

## The golden rule

The audio callback has a hard budget: at 48 kHz with 64-sample blocks that is
~1.3 ms. Miss it once and there is an audible glitch. Therefore, **inside the
callback it is forbidden to**:

- Allocate or free memory (`Box::new`, a growing `Vec::push`, `String`; cloning an
  `Arc` is fine but *dropping* the last `Arc` is not — it triggers `free`).
- Take locks (`Mutex`, `RwLock`), even "just for a moment": priority inversion.
- Make syscalls: `println!`/logging, files, sockets, `thread::sleep`,
  `std::sync::mpsc` channels (they allocate per message).
- `panic!` with formatting (allocates). Use fallback values and report through the
  FIFO.

## Patterns to use instead

### Pre-built commands (the scsynth pattern)

The network thread does ALL the allocation and construction work, and sends the
audio thread a structure that is ready to plug in:

```rust
enum Cmd {
    AddSynth { node: Box<Synth>, target: NodeId, action: AddAction },
    FreeNode { id: NodeId },
    SetControl { id: NodeId, index: u32, value: f32 },
    // ...
}
```

The `Box<Synth>` was built on the network thread. The audio thread pops it from the
FIFO and inserts it into the tree: no `malloc`. To free, the audio thread does NOT
drop: it unlinks the node and pushes it whole through the **garbage FIFO** to the
non-RT thread, which is the one that drops it.

### Ring buffers: `rtrb`

```rust
let (mut producer, mut consumer) = rtrb::RingBuffer::<Cmd>::new(1024);
```

- SPSC, wait-free, no allocations after creation.
- If the command FIFO is full, the network thread waits/retries — never the audio
  thread.
- If the garbage FIFO is full, the audio thread must hold the garbage in a
  pre-allocated list and retry later (never drop).
- Drain the FIFO at the START of each block, with a per-block command limit if the
  worst case needs bounding.

### Shared data: `basedrop`

For data the audio thread reads and the network thread replaces (e.g. the SynthDef
table, a reloaded buffer): `basedrop::Shared` + `Collector`. The audio thread can
drop a `Shared` safely — the actual deallocation happens when the non-RT thread
calls `collector.collect()`.

### Pre-allocated state

- Pools/slabs with fixed capacity decided at startup (max number of nodes, buses,
  buffers — scsynth does exactly this with its command-line options).
- `Vec` is fine if built with `with_capacity` and never exceeded; in debug, guard
  with `assert_no_alloc`.

## cpal: the essentials

```rust
let host = cpal::default_host();
let device = host.default_output_device().expect("no output device");
let config = device.default_output_config()?;
let stream = device.build_output_stream(
    &config.into(),
    move |data: &mut [f32], _| { engine.process(data); },
    move |err| { /* report through a channel, don't panic */ },
    None,
)?;
stream.play()?;
```

Known traps:

- cpal delivers **interleaved frames** (`[L R L R …]`) and the size of `data` **is
  neither fixed nor necessarily a multiple of 64**. The engine must slice: keep an
  internal 64-frame block and an offset, and fill `data` across block boundaries.
- The actual sample rate may differ from the requested one — read
  `config.sample_rate()` and propagate it to every UGen (don't hardcode
  44100/48000).
- On Linux, raw ALSA has variable latencies; with JACK/PipeWire-JACK the behavior
  is more stable at low latencies. Don't assume a backend.
- `Stream` is not `Send` on every platform: create the stream on the thread that
  owns it and keep it alive (if dropped, the audio stops).

## The callback, step by step

```text
for each 64-sample block:
  1. drain the command FIFO (insert/remove nodes, set controls)
  2. execute scheduled bundles whose timestamp falls inside this block
     (split the block at the event offset if sample-accuracy is needed)
  3. clear the audio buses (or copy hardware inputs into the input buses)
  4. walk the node tree in order and call each synth's process()
  5. copy the first N buses to the hardware output
  6. push freed nodes onto the garbage FIFO
```

## Denormals

Filters with tails decaying to zero produce denormal numbers that can be 100x
slower. Enable flush-to-zero at the start of the callback on x86:

```rust
// once per callback, audio thread
unsafe {
    use std::arch::x86_64::{_mm_setcsr, _mm_getcsr};
    _mm_setcsr(_mm_getcsr() | 0x8040); // FTZ + DAZ
}
```

(or add a tiny DC offset, 1e-18, inside the filters — classic scsynth technique).

## Verification

- Wrap the callback body with `assert_no_alloc::assert_no_alloc(|| …)` in builds
  with the test feature enabled.
- Measure callback time (with `Instant`, which on Linux is a VDSO
  `clock_gettime`, acceptable) and publish peak/average CPU through the FIFO —
  this feeds `/status.reply` just like in scsynth.
