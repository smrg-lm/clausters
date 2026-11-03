"""Steps carried out against a server, through the crate's runner.

A verb of the shared crate answers **steps** -- a message to send, a ``/done``
the rest waits for, a barrier -- and `run_steps` is the one walk of them in this
client: a roll's playback goes through it. Which reply releases what is the
runner's (`clausters._native.StepRunner`), as it is the page's and the GUI
host's; what is left here is a socket and waiting on it.

The steps that work on files (a render, a document stored, a file moved) are
the GUI host's application's, which holds every editor that saves or renders;
none reaches a client, and the walk refuses one, naming it, rather than doing
on this machine's disk what the application does on its own.
"""

from array import array

from .base import _osclib
from .errors import CommandError

#: The steps only the application that holds an editor carries out.
_THE_APPLICATIONS = ("render", "store", "move", "measure")


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
        CommandError: when the server refuses a step, answers one with
            something it does not wait on, or a step that works on files
            arrives. Whatever ends the walk early (that, a timeout) retires
            the steps it did not send, so the runner is left idle for the next
            batch.
    """
    runner.call("push", to=to, steps=steps)
    try:
        while True:
            ready = runner.call("ready")
            messages = ready.get("messages", [])
            awaiting = ready.get("awaiting")
            if awaiting is None:
                for message in messages:
                    server.send_msg(message["addr"], *[step_arg(a) for a in message["args"]])
                return
            held = [kind for kind in _THE_APPLICATIONS if kind in awaiting["step"]]
            if held:
                raise CommandError(
                    f"a {held[0]} step is carried out by the application that "
                    "holds the editor, never by a client")
            if not messages:
                raise CommandError("the runner waits on a step nothing was sent for")
            *first, last = messages
            expect = (("/server_sync.reply",) if "sync" in awaiting["step"]
                      else ("/done", "/fail"))
            for message in first:
                server.send_msg(message["addr"], *[step_arg(a) for a in message["args"]])
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
    except BaseException:
        # **A walk given up on leaves nothing behind**: what it did not send,
        # the step it waited on included, is retired, so the next batch on
        # this runner sends nothing of this one.
        runner.call("retire")
        raise


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
