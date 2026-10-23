"""Steps carried out against a server, through the crate's runner.

A verb of the shared crate answers **steps** -- a message to send, a ``/done``
the rest waits for, a barrier -- and `run_steps` is the one walk of them in this
client: a multitrack's playback and a session's load go through it. Which reply
releases what is the runner's (`clausters._native.StepRunner`), as it is the
page's and the GUI host's; what is left here is a socket and waiting on it.
"""

from array import array

from .base import _osclib
from .errors import CommandError


def run_steps(server, runner, steps: list, *, to: str = "sound",
              timeout: "float | None" = None) -> None:
    """Carry ``steps`` out on ``server`` through ``runner``.

    What may go out is sent; where something is awaited, the runner puts the
    message it waits on last, and that one is sent as a request -- so the reply
    cannot arrive before anyone is listening for it -- and its reply is handed
    back, which releases the rest.

    Args:
        server: the server the steps go to.
        runner: a `clausters._native.StepRunner`.
        steps: the steps, as the crate answered them.
        to: which server the runner addresses them to, ``"sound"`` or
            ``"samples"``; a client's is one server either way.
        timeout: how long each awaited reply may take.

    Raises:
        CommandError: when the server refuses a step, or answers one with
            something it does not wait on.
    """
    runner.call("push", to=to, steps=steps)
    while True:
        ready = runner.call("ready")
        messages = ready.get("messages", [])
        awaiting = ready.get("awaiting")
        if awaiting is None or "render" in awaiting["step"]:
            for message in messages:
                server.send_msg(message["addr"], *[step_arg(a) for a in message["args"]])
            if awaiting is None:
                return
            # **An offline render**: nothing is sent for it; this client
            # renders the score into its file and says how it went.
            try:
                render_step(awaiting["step"]["render"])
                failed = None
            except Exception as error:  # noqa: BLE001 -- the runner is told, then it is raised
                failed = error
            answer = runner.call("rendered", **({} if failed is None else {"error": str(failed)}))
            if failed is not None:
                raise CommandError(f"the render into {awaiting['step']['render']['path']} failed: {failed}")
            if answer.get("reply") != "released":
                raise CommandError("the runner did not wait on the render it asked for")
            continue
        if not messages:
            raise CommandError("the runner waits on a step nothing was sent for")
        *first, last = messages
        for message in first:
            server.send_msg(message["addr"], *[step_arg(a) for a in message["args"]])
        expect = (("/server_sync.reply",) if "sync" in awaiting["step"]
                  else ("/done", "/fail"))
        raddr, rargs = server.request(
            last["addr"], *[step_arg(a) for a in last["args"]], expect=expect,
            timeout=timeout)
        answer = runner.call("reply", **{"from": to}, addr=raddr,
                             args=[tagged(a) for a in rargs])
        if answer.get("reply") == "refused":
            raise CommandError(f"{last['addr']} failed: {rargs}")
        if answer.get("reply") != "released":
            raise CommandError(
                f"{last['addr']} was answered by {raddr} {rargs}, "
                "which is not what it waits on")


def render_step(render: dict, progress=None) -> None:
    """**A render step carried out**: the score rendered offline into its file
    by an offline server of its own (a ``--nrt`` process), so two renders run
    at once on two of them. ``progress`` is told the frames done and the frames
    the render comes to."""
    from .render import render_to_file

    # An empty path keeps nothing of the output (`--nrt`'s `-`): the render's
    # work is what its synths write, a `PeaksOut` measuring a file.
    render_to_file(bytes.fromhex(render["score"]), render["path"] or "-", float(render["rate"]),
                   int(render["channels"]), 0, None, render.get("format", "float"),
                   progress=progress)


def step_arg(arg: dict):
    """One step argument as the value `send_msg` encodes to its tag."""
    if "i" in arg:
        return int(arg["i"])
    if "h" in arg:
        return _osclib.Int64(int(arg["h"]))
    if "f" in arg:
        return float(arg["f"])
    if "b" in arg:
        return array("f", arg["b"]).tobytes()
    if "x" in arg:
        return bytes.fromhex(arg["x"])
    return str(arg["s"])


def tagged(value) -> dict:
    """One reply argument in the tagged shape the runner reads -- the other
    direction of `step_arg`. A reply carries ints, floats and strings; anything
    else is handed over as its text."""
    if isinstance(value, bool):
        return {"i": int(value)}
    if isinstance(value, _osclib.Int64):
        return {"h": value.value}
    if isinstance(value, int):
        return {"i": value}
    if isinstance(value, float):
        return {"f": value}
    return {"s": str(value)}
