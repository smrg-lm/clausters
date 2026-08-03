"""The wire a resource handle talks over: which server, and the one shape a
def send has.

A handle built by a constructor (`Synth`, `Group`, `Bus.audio`, `Buffer.alloc`)
carries the server it was created on; one built from a reported id (a
responder, the GUI, the arrangement) may carry none, and falls back to the
ambient server — the same rule `clausters.play` follows. The import is lazy
because `clausters.base.main` reaches back into these modules.
"""

from ..errors import CommandError


def resolve(server=None):
    """``server`` if given, else the ambient one. Raises if none has booted."""
    if server is not None:
        return server
    from ..base.main import main

    return main.resolve_server(None)


def send_def(server, family: str, payload, name: str, wait: bool,
             timeout: float) -> str:
    """Sends one ``/def_send`` message and returns the def's ``name``.

    The shape every family shares — ``family`` is the wire argument that selects
    it (``"synth"``, ``"faust"`` or ``"graph"``): in NRT the send is *scored* at
    time 0 (the renderer loads the def before time advances, so ``wait`` does not
    apply); in RT ``wait=True`` blocks until ``/done``/``/fail`` — raising
    `clausters.errors.CommandError` on the failure — and ``wait=False`` returns
    immediately, to be sequenced with a ``sync`` barrier.

    A carrier that cannot be waited on (``interface.awaitable`` is false — a
    Jupyter kernel's comm, whose reply is queued behind the very cell asking
    for it) drops the confirmation too. The wait was never the barrier: an
    ordered carrier delivers `/def_send` ahead of the `/synth_new` that needs
    it, so what is lost is the early `/fail`, and a def that failed to compile
    shows up as one the server does not have.
    """
    awaitable = getattr(server.interface, "awaitable", True)
    if (getattr(server.interface, "time_mode", "unix") == "score"
            or not wait or not awaitable):
        server.send_msg("/def_send", family, *payload)
        return name
    reply, args = server.request("/def_send", family, *payload, timeout=timeout,
                                 expect=("/done", "/fail"))
    if reply == "/fail":
        raise CommandError(f"/def_send {family} {name!r} failed: {args}")
    return name
