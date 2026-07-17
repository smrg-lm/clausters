# Examples (installed-package)

These examples import `clausters` from the **installed package** — no
`sys.path` shim, no `target/` directory, no separately built binary needed for
the offline one. They are meant to be run after installing the wheel (see the
[client README](../README.md)):

```sh
python -m venv .venv && . .venv/bin/activate
pip install ./clients/python          # builds + bundles the native libs
```

- **`verbs.py`** — the ambient verbs end to end: one `play` for every playable
  kind (event, dict, generator, bare UGen/Faust expression, named def,
  automation, timeline, buffer) and one `render` for the change of state — a
  phrase bounced to a WAV, loaded back and played as a take.

  ```sh
  python clients/python/examples/verbs.py
  ```

- **`embedded.py`** — the same pattern, live from a server running **inside the
  process** (`Session.embed`): the bundled engine, no separate process and no
  socket. Nothing to start.

  ```sh
  python clients/python/examples/embedded.py
  ```

- **`offline_render.py`** — fully self-contained: renders a short arpeggio to a
  WAV through the bundled renderer. No server, no audio device.

  ```sh
  python clients/python/examples/offline_render.py out.wav
  ```

- **`wavetables.py`** — the table family, offline: `/b_gen` wavetables read by
  `vosc` (a lagged `pos` morphs sine into saw) and a `cheby` transfer curve
  driven through `shaper`. No server, no audio device.

  ```sh
  python clients/python/examples/wavetables.py out.wav
  ```

- **`live_udp.py`** — the same pattern, live over UDP to a **separate, running**
  server. The wheel ships that server as the `clausters` command (or use
  `cargo run --release`).

  ```sh
  clausters                                    # standalone server, own process
  python clients/python/examples/live_udp.py   # in another shell
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

- **`timeline_transport.py`** — DAW-style transport over a static `Timeline`:
  capture a pattern into a timeline, edit it, then drive it live with a
  `Playhead` (`play`, `locate`, `loop`, `position`). Needs a **running** server.

  ```sh
  python clients/python/examples/timeline_transport.py
  ```

- **`transport_conductor.py`** — a conductor's play/stop/locate driving two
  clients' playheads in lockstep (`follow_transport` on a **running** server's
  shared transport; sample-exact via `lock_to`). Prints matching positions.

  ```sh
  python clients/python/examples/transport_conductor.py
  ```

- **`gui_editor.py`** — the editor-grade waveform + spectrogram. It **launches
  its own server and GUI** (`Session.live` + `session.gui()`, starting a server
  if none is up and choosing the shared-memory segment for you) and is organized
  as `# %%` cells. Evaluate the cells in VS Code / Jupyter to keep the window
  open and drive `session`/`gui`/`win` between cells (`gui.set(...)`,
  `play_pass()`, `gui.close(win)`); or run it as a plain script, which follows
  the playhead for a while and then closes. The server and GUI binaries are both
  bundled; needs a display and a GPU adapter.

  ```sh
  python clients/python/examples/gui_editor.py    # as a script; or open the cells in VS Code / Jupyter
  ```

- **`plotting.py`** — quick looks with the free-standing `clausters.plot`: each
  call opens **its own window** (the GUI host boots lazily; no audio server —
  defs render through the bundled NRT renderer). A **sequential visual tour**,
  one window at a time, each announced on the console: a `SynthDef`'s output in
  per-channel lanes (view retuned to spectrum and back), an `Env` played by the
  engine's own `EnvGen` (a value-axis side pinned and released), sequences
  (`Pseq`; an arbitrary-range `Pwhite` with the axis auto-fitted), and a
  `GraphDef`'s averaged spectrum with its frequency scale swapped log→mel→log —
  every live change through the returned handle (`win.set(...)`), and each
  window closes before the next appears. `# %%`
  cells; needs a display and a GPU adapter.

  ```sh
  python clients/python/examples/plotting.py     # as a script; or open the cells in VS Code / Jupyter
  ```

The broader catalog of examples (including the low-level transports and the raw
OSC helpers) lives in the repo-root [`examples/`](../../../examples/); those use
a `sys.path` shim so they run straight from a source checkout without an install.
