"""The `env` builders that mirror the wire, re-exported by `env`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/env.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "free_self",
    "pause_self",
    "done",
    "free_self_when_done",
    "pause_self_when_done",
]


def free_self(signal) -> Ugen:
    """Frees the enclosing synth while `signal` is greater than zero, passing
    it through unchanged. The trigger-driven counterpart of a `DoneAction` --
    use it when what ends the note is not the envelope.
    """
    return Ugen("FreeSelf", [signal])


def pause_self(signal) -> Ugen:
    """Pauses the enclosing synth while `signal` is greater than zero, passing
    it through. Resume with `Server.run`; it re-pauses only if the signal is
    still up, so this is a gate rather than a one-way door.
    """
    return Ugen("PauseSelf", [signal])


def done(source) -> Ugen:
    """1 once `source` has finished, 0 before -- a trigger the rest of the
    graph can read.

    `source` must be a ugen that *can* finish (`env_gen`, `line`, `x_line`);
    the server rejects the def by name otherwise. What it reads is the done
    flag, not the value on the wire: an envelope that has played out sits at
    its final level, which tells you nothing.
    """
    return Ugen("Done", [source])


def free_self_when_done(source) -> Ugen:
    """Passes `source` through and frees the synth once it has finished. The
    idiom for an envelope whose own `done_action` is `DoneAction.NONE`
    because something else in the graph still needs it.
    """
    return Ugen("FreeSelfWhenDone", [source])


def pause_self_when_done(source) -> Ugen:
    """Passes `source` through and pauses the synth once, when it has finished;
    a synth resumed after is not paused again.
    """
    return Ugen("PauseSelfWhenDone", [source])
