# Examples

The runnable demos live in the **repository**, not in any published package: a
wheel or an npm install carries the library, not the examples. They come with a
checkout:

| where | what runs there |
|---|---|
| `examples/` | the Rust demos and the Python scripts that drive a server from a source checkout (a `sys.path` shim finds the client; a few build their OSC bytes with nothing but the standard library) |
| `clients/python/examples/` | the installed-package scripts, including the `gui_*` family that drives the GUI host |
| `clients/web/examples/` | the browser pages |
| `examples/*.sh` | the shell demos (they need `oscsend`) |

**Each example documents itself.** Its module docstring — or, for a page, the
comment at the top — says what it shows, what it needs (a running server, a
display, or nothing at all) and how to run it. That is the catalog: the
directory listing plus each file's first paragraph, which cannot drift from the
code the way a second list here would.

How to run each family:

```sh
cargo run --release --example bench          # Rust: cargo run --example <name>
python3 examples/sequencing.py               # root Python: from a checkout, no install

pip install ./clients/python                 # installed-package scripts:
python clients/python/examples/basics/hello_note.py #   ...run from anywhere

cd clients/web && npm install && ./build.sh  # web pages: build, then serve
python3 -m http.server                       #   http://localhost:8000/examples/...
```

Most of the Python demos render **offline** and need no audio hardware and no
server; the ones that drive a live server say so and expect `cargo run
--release` (or the installed `clausters` command) in another terminal.

The examples are also this project's **manual test surface**: new audible or
visual behavior is checked by running one, so an example that exercises a
feature is part of shipping it.

## Python binding (`clients/python/clausters/ipc.py`)

The reusable local-transport library (standard library + `ctypes`): a
shared-memory client and the embed façade with a synchronous `request()` call.
It is the `clausters.ipc` module of the high-level Python client package
(`clients/python/clausters/`), and its `Clausters`/`ShmClient`/`render` are
re-exported from the top-level `clausters` package. See [Local transports &
embedding](ipc.md) for the API.
