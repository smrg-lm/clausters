# Examples (installed-package)

These examples import `clausters` from the **installed package** — no
`sys.path` shim, no `target/` directory, no separately built binary needed for
the offline ones. They are meant to be run after installing the wheel (see the
[client README](../README.md)):

```sh
python -m venv .venv && . .venv/bin/activate
pip install ./clients/python          # builds + bundles the native libs
python clients/python/examples/hello_note.py
```

**Each example documents itself**: its module docstring says what it shows, what
it needs and how to run it. Two good entry points:

- `hello_note.py` — the shortest path to sound: boot a server, play a note.
- `verbs.py` — every playable kind through one `play`, and one `render` for the
  change of state.

Most of them render **offline** — no server, no audio device — and say so. The
ones that need a **running** server (`live_udp.py`, the transport and responder
demos) name it in their docstring; the wheel ships that server as the
`clausters` command. The `gui_*` family drives the GUI host and needs a display
and a GPU adapter; several of those are organized as `# %%` cells, so the window
stays open while you evaluate cell by cell in VS Code or Jupyter.

The lower-level demos — the transports, the raw OSC helpers, the audible tours
of the UGen families — live in the repository-root
[`examples/`](../../../examples/); those use a `sys.path` shim so they run
straight from a source checkout without an install.
