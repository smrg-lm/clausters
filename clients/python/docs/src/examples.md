# Examples

The installed-package examples import `clausters` from the installed package — no `sys.path` shim, no `target/` directory, no separately built binary for the offline one. Run them after installing the package (see [Getting started](getting-started.md)):

```sh
python -m venv .venv && . .venv/bin/activate
pip install ./clients/python          # builds + bundles the native libs
```

- **`offline_render.py`** — fully self-contained: renders a short arpeggio to a WAV through the bundled embed renderer. No server, no audio device.

  ```sh
  python clients/python/examples/offline_render.py out.wav
  ```

- **`live_udp.py`** — the same pattern, live over UDP to a **running** server (start one with `cargo run --release`, or the installed `clausters` binary).

  ```sh
  python clients/python/examples/live_udp.py
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

The first two share their pattern code and differ only in the `Server` interface — the seam from [The client, layer by layer](guide.md) in practice.

The broader catalog of examples (the low-level transports and the raw OSC helpers) lives in the repository-root `examples/`; those use a `sys.path` shim so they run straight from a source checkout without an install.
