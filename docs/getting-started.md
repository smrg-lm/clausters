# Getting started

This chapter takes you from a checkout to a sound: build the server, run it, play a note over OSC, and render a score offline. (This is the English, user-facing path; the Spanish `GUIA.md` is a maintainer QA checklist that walks every feature.)

## Requirements

- A recent stable Rust toolchain (`rustup`, edition 2024).
- On Linux, the ALSA development headers for the default real-time/MIDI backends (`libasound2-dev`).
- Optional, only for the matching feature:
  - `pipewire` (native PipeWire audio backend): `libpipewire-0.3-dev` and `clang`.
  - `midi-jack` (route live MIDI through JACK, needed on PipeWire systems): `libjack-jackd2-dev`.
  - `faust` (libfaust embedding): `libfaust` built with the LLVM backend — see [Contributing](contributing.md).

  The core builds and runs without any of them. On Ubuntu 26.04:

  ```sh
  # default build (ALSA)
  sudo apt install build-essential libasound2-dev
  # extras per feature
  sudo apt install libpipewire-0.3-dev clang   # --features pipewire
  sudo apt install libjack-jackd2-dev          # --features midi-jack
  ```

## Build

```sh
cargo build --release
```

The default feature `realtime` pulls in the [cpal](https://crates.io/crates/cpal) audio backend. To build the engine without an audio device (CI, tests, offline rendering only), disable it: `cargo build --no-default-features`.

## Run the server

```sh
cargo run --release
```

It opens the audio device and listens for OSC on UDP `127.0.0.1:57110`, printing a one-line banner. It is **silent until you create a synth**. Stop it with `/quit` or Ctrl-C. Useful flags:

```sh
cargo run --release -- --workers 3        # DSP threads for /g_parallel groups
cargo run --release -- --shm /dev/shm/clausters   # shared-memory transport
```

## Play a sound

The server speaks OSC, so any OSC client works. The bundled [`osc_ping`](examples.md) example is the quickest — in a **second terminal**, with the server running:

```sh
cargo run --example osc_ping -- beep      # default synth at 440 Hz, retuned, freed
cargo run --example osc_ping -- map       # /n_map + /n_mapa demo (control & audio buses)
cargo run --example osc_ping -- status quit
```

By hand with [`oscsend`](https://man.archlinux.org/man/oscsend.1) (from `liblo`):

```sh
oscsend localhost 57110 /s_new siii default 1000 1 0   # play "default" at the root
oscsend localhost 57110 /n_set isf 1000 freq 330       # retune it
oscsend localhost 57110 /n_free i 1000                 # stop it
```

`default` is a built-in sine def; define your own with `/d_recv` (see [Defs, UGens & the OSC protocol](schemas.md)).

> Sandbox note for the test harness: some environments isolate the network between shell invocations, so a server started in one invocation is unreachable from the next. Run the server and client in the **same** invocation there (server in the background with `&`, then the client). See [Contributing](contributing.md).

## Render a score offline (NRT)

No audio device needed — the same engine renders a score to WAV:

```sh
python3 examples/json_client.py score    # writes /tmp/clausters_score.osc
cargo run --release -- --nrt /tmp/clausters_score.osc /tmp/out.wav
```

A score is the scsynth binary format: length-prefixed OSC bundles whose timetags count seconds from the start of the render. Options: `--rate`, `--channels`, `--format int16|int24|float`, `--workers`. See the *NRT mode* section of [Defs, UGens & the OSC protocol](schemas.md).

## Where to go next

- Control the server in depth: [Defs, UGens & the OSC protocol](schemas.md).
- Timing: [timed bundles & the sample clock](sample-clock.md).
- Ordering and cores: [auto-sorted](auto-order.md) and [parallel](parallel.md) groups.
- Embed it in your own program: [Using Clausters as a library](using-as-a-library.md).
- Browse runnable demos: [Examples](examples.md).
