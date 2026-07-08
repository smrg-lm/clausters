# Examples

Runnable demos live in `examples/` (Rust and Python) and `clients/python/`. Unless noted, the Python ones need the server running first (`cargo run --release`) and use the standard library only.

## Rust examples (`cargo run --example <name>`)

| example | what it shows | run |
|---|---|---|
| `osc_ping` | Minimal OSC client for manual testing: subcommands `status`, `beep`, `vibrato`, `map` (the `/n_map`/`/n_mapa` demo), `quit`. | `cargo run --example osc_ping -- beep` |
| `bench` | Graph-throughput benchmark (offline): how many copies of a graph fit in real time at 48 kHz, plus the parallel-group speedup. With `--features faust`, an apples-to-apples UGen-vs-Faust section runs the *same* DSP (the parity-test sine) through both engines to isolate per-synth audio-loop overhead. | `cargo run --release --example bench` |
| `stress` | Single-core stress test against a **running** server (the real-time complement to `bench`, which is offline): ramps nodes of an `n`-sinusoid def while watching the server's own CPU meter and late-block counter in `/status.reply`, and reports the last stable count before the peak load crosses `--limit` or a block runs late. Two axes: `--sines` (DSP weight per node) and the ramp step (per-node engine overhead). | `cargo run --release --example stress -- --sines 10` |
| `render_golden` | Regenerates the golden reference WAVs in `tests/golden/` from the shared scenes — run it and **listen** before committing. | `cargo run --example render_golden` |

## Python clients (`examples/`)

| script | what it shows |
|---|---|
| `sequencing.py` | The high-level [`clausters` client](clients.md): pattern sequencing with `Session` + `Pbind` + value patterns, and the one seam that runs the *same* phrase offline (NRT render) or live (UDP) by swapping the session. The flagship intro to the client library. |
| `synthdef.py` | Builds a UGen `SynthDef` from Python (lowercase callables → graph → `/d_recv`), instance-based with no global build context, and proves it renders **byte-identically** to the server's built-in `default` def. |
| `tcp_client.py` | The same `Server` facade over **TCP** (`OscTcpInterface`, length-prefixed OSC; start the server with `--tcp`) instead of UDP. |
| `midi_file.py` | Renders an event pattern to a **Standard MIDI File** (`.mid`, or `--clip` for a 16-bit-velocity MIDI 2.0 clip): the *same* `Pbind`/`TempoClock` targets a `MidiServer` destination instead of the OSC `Server` (double dispatch), realizing each note as MIDI on/off and writing the file through the `clausters-midi` crate. No server, no audio. |
| `midi_live.py` | Plays the same pattern **live** out a virtual OS MIDI port (`MidiRtInterface`, `clausters-midi --features live`): note-ons at the beat, note-offs scheduled — route it to a synth or the server's own `--midi` input with `aconnect`. |
| `json_client.py` | Generates defs as JSON and drives the server over OSC. Subcommands: `status`, `ugen`, `faust` (box API), `signal` (Faust Signal API: a `recursion`/`self` sine + a one-pole lowpass on noise), `wavetable`, `buffer`, `bundle`, `feedback` (a LocalIn/LocalOut resonant comb), `score` (writes an NRT score to `/tmp/clausters_score.osc`). |
| `auto_order.py` | [Auto-sorted groups](auto-order.md): builds a source → fx → master chain reversed on purpose and repairs it with one `/g_sortMode`. |
| `group_set.py` | scsynth **group semantics**: three voices share a group and one `/n_set` on the group ramps all of them at once (the parameter propagates down the subtree). |
| `graphdef.py` | A **GraphDef** ([node-graph program](schemas.md#graphdef-d_graph-graph_new--node-graph-programs)): a two-oscillator voice wired through a private internal bus, built with the `GraphDef` builder and driven through its **named surface** — one `freq` port mapping to both oscillators (the second scaled to a fifth), which a bare group `/n_set` cannot do. |
| `graphdef_poly.py` | A **polyphonic** GraphDef: a shared mixer (instantiated once with `server.graph`) plus a per-voice oscillator marked `voice=True`, spawned per note with `server.graph_voice` — the same shared/per-voice model a `/midi_bind` to a GraphDef uses. |
| `sample_clock.py` | [The sample clock](sample-clock.md) as master timebase: models `sample(t)` from `/clock` anchors and schedules with `/sched`. |
| `shm_client.py` | The shared-memory transport (`--shm`): the same OSC with no sockets anywhere. |
| `clock_recorder.py` | The shared-memory sample clock made checkable: reads `ShmClient.clock` directly (no round trip), schedules a pristine `Impulse` **exactly every N samples** with `/sched`, records the real output (`pw-record`) and reports the impulse spacing/jitter/drift. Duration is free (`--seconds`), seconds to hours. |
| `embed_render.py` | Synchronous offline render through the embed C ABI (`ctypes`), no server process. |

The OSC encoding/decoding in these scripts is hand-rolled (stdlib only); they double as a compact reference for the wire format.

## Installed-package examples (`clients/python/examples/`)

These import `clausters` from the **installed package** (no `sys.path` shim, no `target/`), so they show the wheel workflow: `pip install ./clients/python` then run from anywhere. See the [client README](clients.md#distribution).

| script | what it shows |
|---|---|
| `offline_render.py` | Fully self-contained: a `Session.nrt` `Pbind` rendered to a WAV through the **bundled** embed renderer — no server, no audio device, no source checkout. |
| `live_udp.py` | The same pattern live over UDP to a running server (`Session.live`), proving the offline/live seam from an installed package. |

## Shell

| script | what it shows |
|---|---|
| `persistence.sh` | [Def persistence](schemas.md#persisting-defs-across-restarts): `/d_faust` a def with `--data-dir`, quit, then restart and instantiate it **without re-sending** — it reloaded from disk (with its bitcode cache). Needs the `faust` feature and `oscsend`. |
| `midi_standalone.sh` | [MIDI-standalone](schemas.md#midi-standalone-bindings--boot-preset): set up a SynthDef + a GraphDef + a `/midi_bind` once, quit, then restart with `--midi` and the binding is **back with no OSC** — play it from a controller via `aconnect`. Needs `oscsend`. |

## Python binding (`clients/python/clausters/ipc.py`)

The reusable local-transport library (standard library + `ctypes`): a shared-memory client and the embed façade with a synchronous `request()` call. It is the `clausters.ipc` module of the high-level Python client package (`clients/python/clausters/`), and its `Clausters`/`ShmClient`/`render` are re-exported from the top-level `clausters` package. See [Local transports & embedding](ipc.md) for the API.
