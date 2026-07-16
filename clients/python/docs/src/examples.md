# Examples

The installed-package examples import `clausters` from the installed package — no `sys.path` shim, no `target/` directory, no separately built binary for the offline one. Run them after installing the package (see [Getting started](getting-started.md)):

```sh
python -m venv .venv && . .venv/bin/activate
pip install ./clients/python          # builds + bundles the native libs
```

- **`hello_note.py`** — the shortest path to sound: `Server.boot()` then `play(Event(...))`, no `Session` and no clock wiring. The booted server becomes the default session, so a bare event plays immediately and frees itself. Start here.

  ```sh
  python clients/python/examples/hello_note.py
  ```

- **`verbs.py`** — the ambient verbs end to end: one `play` for every playable kind (event, dict, generator, bare UGen/Faust expression, named def, automation, timeline, buffer) and one `render` for the change of state — a phrase bounced to a WAV, loaded back and played as a take. See [The ambient verbs](verbs.md).

  ```sh
  python clients/python/examples/verbs.py
  ```

- **`embedded.py`** — the same pattern, live from a server running **inside the process** (`Session.embed`). Nothing to start: it opens the bundled engine in-process and plays. The batteries-included path. No server process, no socket.

  ```sh
  python clients/python/examples/embedded.py
  ```

- **`offline_render.py`** — fully self-contained: renders a short arpeggio to a WAV through the bundled renderer. No server, no audio device.

  ```sh
  python clients/python/examples/offline_render.py out.wav
  ```

- **`typed_controls.py`** — also fully self-contained (renders a WAV, no server): a `SynthDef` whose controls show the three control **types** — a lagged `freq` (portamento glide), a `gate` **trigger** (`rate="tr"`, re-plucking a percussive envelope each set), and a `detune` drawn once with `rand` (an `ir` scalar). A `Routine` sets `freq` / `gate` per note through timetagged bundles. See [Building defs](defs.md#control-types-and-rates).

  ```sh
  python clients/python/examples/typed_controls.py out.wav
  ```

- **`graph_maths.py`** — also self-contained (renders a WAV, no server): a `SynthDef` that does real per-sample **maths** — `.midicps()` for pitch, `.distort()` for timbre, a comparison-and-`.clip2()` tremolo — all composing the server's generic operator UGens (S3), bit-identical to the same maths computed off the RT path. See [Maths on a UGen graph](defs.md#maths-on-a-ugen-graph).

  ```sh
  python clients/python/examples/graph_maths.py out.wav
  ```

- **`spectral.py`** — also self-contained (renders a WAV, no server): a frequency-domain `fft` → `pv_brick_wall` → `ifft` chain (S8) that low-passes white noise **in the spectral domain**, rendered next to the raw noise so the effect is audible. No buffer is allocated; the frame is synth-private scratch on the server. See [The frequency-domain chain](defs.md#the-frequency-domain-chain).

  ```sh
  python clients/python/examples/spectral.py out.wav
  ```

- **`live_udp.py`** — the same pattern, live over UDP to a **separate, running** server. The wheel ships that server as the `clausters` command (or use `cargo run --release`).

  ```sh
  clausters                                    # the standalone server, in its own process
  python clients/python/examples/live_udp.py   # in another shell
  ```

- **`transport_sync.py`** — two independent clients lock to a **running** server's sample clock and join its shared transport, so a `quant`-ed routine on each lands on the same bar (sample-aligned). Prints the matching next-bar sample and plays a note on each. See [Timing models](timing-models.md).

  ```sh
  python clients/python/examples/transport_sync.py
  ```

- **`osc_responder.py`** — the client as an OSC hub: an `OscFunc` relays incoming `/note` messages to a **running** server as synths, and an `OscFunc` on `/transport.reply` re-aligns when a conductor changes the shared transport. Self-feeds a few messages to demonstrate. See [Receiving OSC and MIDI](responders.md).

  ```sh
  python clients/python/examples/osc_responder.py
  ```

- **`midi_responder.py`** — a `MidiFunc` turning a MIDI keyboard into synths on a **running** server: note-on starts a synth, note-off frees it. Opens a virtual MIDI input port (`clausters-in`) to wire a source into. Needs the `live` cdylib (`cargo build --release -p clausters-midi --features live`).

  ```sh
  python clients/python/examples/midi_responder.py
  ```

- **`timeline_transport.py`** — DAW-style transport over a static `Timeline`: captures a pattern into a timeline, edits it, then drives it live with a `Playhead` — `play`, `locate` (seek), `loop`, and a song `position`. Needs a **running** server. See [Timelines and the playhead](timelines.md).

  ```sh
  python clients/python/examples/timeline_transport.py
  ```

- **`transport_conductor.py`** — a conductor's play/stop/locate driving several clients' playheads **in lockstep**: two followers `follow_transport` a **running** server's shared transport and roll together (sample-exact, since they also `lock_to`). Prints their matching song positions. See [A DAW-style transport](transport.md).

  ```sh
  python clients/python/examples/transport_conductor.py
  ```

The first three share their pattern code and differ only in the `Server` interface — embedded, offline or live — the seam from [The client, layer by layer](guide.md) in practice.

The broader catalog of examples (the low-level transports and the raw OSC helpers) lives in the repository-root `examples/`; those use a `sys.path` shim so they run straight from a source checkout without an install.

The GUI examples (`gui_*.py`) drive the **clausters GUI host** — a separate process (or a browser tab) this client talks to over the same widget protocol. The host itself, including the **browser quick-start** (the wasm bundle, `--ws`, and the streamed-meters and fetched-bulk data paths), is documented in the [Clausters server book](https://clausters.readthedocs.io/)'s clients chapter. Two of them close the composition loop: `gui_multitrack.py` lays out tracks of clips on one shared time axis, and `gui_composer.py` drives that view from a composition — the arrangement drawn, dragged and re-rendered (see [Composition: the arrangement and the multitrack editor](composition.md), and the step-by-step tutorial [Composing a piece](composing.md), which builds the same piece interactively).
