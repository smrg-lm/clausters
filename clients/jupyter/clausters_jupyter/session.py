"""`notebook`: the one function that knows a notebook is involved.

Everything else in this package is plumbing under it, and everything above it
is the ordinary client. After this call the verbs draw in cells:

```python
import clausters_jupyter

plot(sine(440) * 0.5)
scope(bus=0)
```

`notebook` returns a plain `clausters.Session` — not a subclass, nothing
added — because the notebook changes only *where the bytes go*. Patterns,
routines, `render`, the `TempoClock` and the `with` block are untouched, and a
script written against a desktop host runs here unchanged.

What it actually does is four registrations, and every one of them goes through
a seam the ordinary client already has — there is no notebook-shaped hole in
`clausters.Session` and nothing here reaches into one:

- builds a `clausters_jupyter.bridge.Bridge` and takes its carriers,
- hands them to a `clausters.gui.GuiHost` (and, for the in-page backend, to a
  `clausters.defs.Server`) through the ``interface=`` seam those two already
  have,
- installs that host on the session with `clausters.Session.adopt_gui` — the
  seam for a host the session could not have booted itself, this one living at
  the far end of a kernel's comm — and registers it with
  `clausters.gui.set_ambient_host`, which is what makes `plot` and `scope`
  resolve it instead of booting a desktop process,
- makes the session ambient with `clausters.Session.activate`, since a
  notebook's cells each run on their own and there is no ``with`` block for
  them to be inside of.

**The two backends differ in capability, not in comfort.** `page` runs the GUI
host and the engine in the cell as wasm: it works with a remote kernel and
sounds where you are looking, and it has no Faust (the in-page engine is the
``synth,embed`` build — no libfaust, no LLVM) and no shared memory or mmap, so
meters and scopes read over the wire. `native` boots a local server with its
full capability (Faust, shared memory, every device the machine has) and draws
its GUI in the cell.

`native` is local-only, and by two separate constraints rather than one: the
audio comes out of the kernel's machine, and the host in the page opens its
audio leg to the server's ``--ws`` port from the *browser*, which only reaches
a kernel on the same machine. Neither is detectable from here — a remote kernel
looks exactly like a local one — so this is documented rather than enforced.
What it looks like when it is wrong: a notebook that draws, and is silent, and
whose meters never move.
"""

import dataclasses

from clausters import Session, gui as gui_module
from clausters.base import IdShare
from clausters.defs import Server, ServerOptions

from . import formatters
from .bridge import Bridge
from .carrier import SERVER_CHANNEL
from .widget import ClaustersWidget

__all__ = ["notebook", "current", "audio"]

#: The server's WebSocket port when ``--ws`` is passed without one. The native
#: backend's host opens its audio leg here, from the *browser*, so the two ends
#: have to agree on a number nobody typed.
DEFAULT_WS_PORT = 57120

#: The kernel's half of the id space, the page holding the other.
#:
#: Both ends author against one engine here -- the kernel sends the defs and
#: the nodes over its comm, and the page holds a `clausters.Session` on the
#: very same in-page engine -- so their allocators would otherwise start at the
#: same base and hand out the same first id. The split needs no agreement
#: beyond the index each side is given: the kernel takes 0, and the page's
#: front end takes 1 (`PAGE_SHARE` in `notebook/widget.ts`). It costs half the
#: range of each space, which is thousands of live nodes either way.
KERNEL_SHARE = IdShare(0, 2)

#: The session `notebook` last built, so a second call is a no-op rather than a
#: second host over the same page.
_current = None
#: Its `clausters_jupyter.bridge.Bridge`, and whether importing the package is
#: what built it (see `_replaceable`).
_bridge = None
_autowired = False


def current():
    """The notebook session in force, or ``None`` before `notebook` is called."""
    return _current


def _replaceable(backend: str) -> bool:
    """Whether the session in force may be thrown away for a `notebook` call
    asking for something else.

    Importing the package wires the default session, which is the point — but
    it means an explicit ``notebook("native")`` runs *second*, against a
    session that already exists, and silently returning that one would give the
    caller the backend they did not ask for. It is safe to replace because the
    auto-wired session costs nothing until a window is displayed: no wasm is
    loaded, no process is started, no comm is opened. Once a cell is showing
    one, it is not replaceable and asking is an error rather than a no-op.
    """
    if not _autowired or _bridge is None:
        return False
    if _bridge.has_engine == (backend == "page"):
        return False                      # same backend: nothing to replace
    return not _bridge.showing()


def notebook(backend: str = "page", *, width: int = 480, height: int = 420,
             server: "Server | None" = None,
             options: "ServerOptions | None" = None,
             server_url: "str | None" = None, _autowiring: bool = False,
             **session_kw) -> Session:
    """Wire the client to draw in notebook cells, and return its `Session`.

    Args:
        backend: ``"page"`` (default) — the GUI host and the audio engine both
            run in the cell as wasm; the only backend that works with a remote
            kernel. ``"native"`` — a local `clausters` server with its full
            capability, its GUI drawn in the cell.
        width, height: the default canvas size for a window's cell. A window
            resizes itself to its output area afterwards, so this is the first
            frame rather than a constraint.
        server: an already-built `clausters.defs.Server` to use instead of the
            one this would make. For ``"native"``, the way to use a server you
            booted yourself — it must have been booted with ``--ws``, or pass
            ``server_url=""`` and give up bound widgets.
        options: `clausters.defs.ServerOptions` for the ``"native"`` server this
            boots. ``ws`` is forced on: the host in the page reaches the server
            over a WebSocket and there is no other carrier a browser can use.
        server_url: the ``ws://`` URL the in-page host opens its audio leg to
            (``"native"`` only). ``None`` derives it from the server's address;
            ``""`` leaves the leg unconnected, which costs the bound widgets —
            a meter, a scope, a slider driving a running node — and nothing
            else.
        session_kw: passed to `clausters.Session`.

    Returns: the `clausters.Session`, also installed as the ambient one, so the
    free-standing verbs resolve it without being told.
    """
    global _current, _bridge, _autowired
    if _current is not None:
        if _replaceable(backend):
            _current, _bridge = None, None
        elif _bridge is not None and _bridge.has_engine != (backend == "page"):
            raise RuntimeError(
                f"this notebook is already running the "
                f"{'page' if _bridge.has_engine else 'native'} backend and a "
                f"cell is showing it; {backend!r} would need a second host for "
                "this one notebook, whose windows, journal and ids all belong "
                "to the first. Choose the backend in the first cell, before "
                "anything draws - or restart the kernel. (Another *notebook* "
                "in the same tab is unaffected: it gets a host of its own.)")
        else:
            return _current
    if backend not in ("page", "native"):
        raise ValueError(f"unknown backend {backend!r} (page or native)")
    if backend == "page" and (options is not None or server_url is not None):
        raise ValueError(
            "options= and server_url= belong to backend='native': the in-page "
            "engine is not launched and is not reached over a socket")

    engine = backend == "page"
    ws_port = DEFAULT_WS_PORT
    if server is None and not engine:
        server, ws_port = _boot_native(options)
    if server_url is None:
        server_url = "" if engine else f"ws://{server.target.host}:{ws_port}"

    bridge = Bridge(lambda: ClaustersWidget(
        engine=engine, server_url=server_url, session=bridge.session,
        width=width, height=height), engine=engine)
    # The page holds a host client of its own over the same wasm host, so the
    # widget ids are split even under the native backend, where the audio ids
    # are not (there the kernel's server is a process the page never authors
    # against).
    host = gui_module.GuiHost(interface=bridge.carrier(), share=KERNEL_SHARE)
    # The page shares no filesystem with the kernel, so a bulk payload cannot
    # ride as a path the host maps -- it travels as a blob beside the message.
    # With a remote kernel this is not a nicety but the only truth available.
    # It holds for the native backend too: the host is in the page either way,
    # and the file the server writes is on the kernel's disk.
    host.local_files = False

    if server is None:
        server = Server(interface=bridge.carrier(SERVER_CHANNEL),
                        share=KERNEL_SHARE)

    session = Session(server, **session_kw)
    session.adopt_gui(host)               # the host is built; nothing to boot
    gui_module.set_ambient_host(host)
    formatters.unregister()               # a replaced session leaves none behind
    formatters.register(bridge)
    # Ambient for good, not for a block: a notebook's cells each run on their
    # own, so there is no `with` to be inside of.
    session.activate()
    _current, _bridge, _autowired = session, bridge, _autowiring
    return session


def _boot_native(options: "ServerOptions | None") -> "tuple[Server, int]":
    """Start the local server the ``"native"`` backend talks to, and say on
    which port the page will find it.

    ``ws`` is forced on rather than merely defaulted, because it is not a
    preference here: the GUI host runs in the page, and a browser can open a
    WebSocket and nothing else. Without it the host has no audio leg, so a
    meter shows nothing and a bound slider moves nothing — the kind of failure
    that looks like a bug in the widget.
    """
    options = ServerOptions() if options is None else options
    if not options.ws:
        options = dataclasses.replace(options, ws=True)
    port = options.ws if options.ws is not True else DEFAULT_WS_PORT
    return Server.boot(options=options), int(port)


def audio():
    """The cell that carries the in-page audio engine — display it to hear.

    ```python
    import clausters_jupyter
    clausters_jupyter.audio()
    ```

    The engine runs in the page, so it needs a cell of its own when nothing
    else is on screen. Asking is rarely necessary — the first audio a notebook
    sends with nothing displayed puts this same cell up on its own, so that
    creating a synth is what starts the sound — but asking places it where you
    want it rather than under whichever cell got there first. A notebook that
    plots needs neither: any displayed window carries the same leg to the same
    engine.

    A browser still starts no audio until something in the page is clicked.
    """
    if _current is None:
        notebook()
    link = _bridge
    if not link.has_engine:
        raise RuntimeError(
            "backend='native' has no in-page engine to carry: the audio comes "
            "out of the kernel's machine, not the browser. This cell would "
            "show an empty canvas and connect nothing.")
    return link.audio_widget()
