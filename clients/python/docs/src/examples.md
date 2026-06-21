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

The two share their pattern code and differ only in the `Server` interface — the seam from [The client, layer by layer](guide.md) in practice.

The broader catalog of examples (the low-level transports and the raw OSC helpers) lives in the repository-root `examples/`; those use a `sys.path` shim so they run straight from a source checkout without an install.
