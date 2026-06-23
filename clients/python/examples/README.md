# Examples (installed-package)

These examples import `clausters` from the **installed package** — no
`sys.path` shim, no `target/` directory, no separately built binary needed for
the offline one. They are meant to be run after installing the wheel (see the
[client README](../README.md)):

```sh
python -m venv .venv && . .venv/bin/activate
pip install ./clients/python          # builds + bundles the native libs
```

- **`offline_render.py`** — fully self-contained: renders a short arpeggio to a
  WAV through the bundled embed renderer. No server, no audio device.

  ```sh
  python clients/python/examples/offline_render.py out.wav
  ```

- **`live_udp.py`** — the same pattern, live over UDP to a **running** server
  (start one with `cargo run --release`, or the installed `clausters` binary).

  ```sh
  python clients/python/examples/live_udp.py
  ```

- **`transport_sync.py`** — two clients lock to a **running** server's sample
  clock and join its shared transport, landing a `quant`-ed routine on the same
  bar.

  ```sh
  python clients/python/examples/transport_sync.py
  ```

- **`osc_responder.py`** — the client as an OSC hub: an `OscFunc` relays incoming
  `/note` to a **running** server, and another reacts to `/transport.reply`
  pushes. Self-feeds a few messages to demonstrate.

  ```sh
  python clients/python/examples/osc_responder.py
  ```

- **`midi_responder.py`** — a `MidiFunc` turning a MIDI keyboard into synths on a
  **running** server (note-on starts a synth, note-off frees it). Opens a virtual
  MIDI input port (`clausters-in`); needs the `live` cdylib (`cargo build
  --release -p clausters-midi --features live`).

  ```sh
  python clients/python/examples/midi_responder.py
  ```

The broader catalog of examples (including the low-level transports and the raw
OSC helpers) lives in the repo-root [`examples/`](../../../examples/); those use
a `sys.path` shim so they run straight from a source checkout without an install.
