"""The environment: an isolated place to make sound.

An `Environment` is the unit of isolation — a `server`, its clock(s), and its
own random context. **Both the default session (`clausters.base.main.Main`) and
an explicit `clausters.Session` are Environments**: the default session is
simply the one used when none is named, and a named session is the same kind of
thing with its own state. That is what lets several coexist — a live take next
to an offline render, each reproducible on its own seed — without touching each
other.

This base carries only what every environment shares: the seedable random
context (`RandomContext`) and the `server` slot. The process-wide ambient
resolution (which environment a free-standing play belongs to) and the
thread-local execution registry live on the default session (`Main`); the
driving surface (`play`/`render`/`run`, the factories) lives on `Session`.
"""

import os


class RandomContext:
    """One seedable RNG root (the shared native `clausters._native.Rng`).

    Each environment is its own random context, so each reproduces
    **independently**: ``seed(n)`` on one never touches another's stream. A
    `clausters.base.stream.Stream` created while a context is active derives its
    own generator from that context's root (see `clausters.base.rand`), so two
    sessions stay reproducible per session regardless of interleaving.
    """

    def __init__(self):
        self._rng = None      # lazy: creating it loads the native core
        self._seed = None

    def seed(self, value=None):
        """Seeds this context's RNG (None reseeds from entropy). Returns the
        seed actually used so the context can be reproduced."""
        from .. import _native

        if value is None:
            value = int.from_bytes(os.urandom(8), "little")
        self._seed = value
        self._rng = _native.Rng(value)
        return value

    @property
    def rng(self):
        """The context value stream (`clausters._native.Rng`, the shared
        core generator — reproducible across client languages). Created lazily,
        seeded from entropy unless `seed` was called."""
        if self._rng is None:
            self.seed(self._seed)
        return self._rng


class Environment(RandomContext):
    """A place things play: a `server` plus a random context (and, on a
    `clausters.Session`, clock(s) and a driving surface).

    The shared base of the default session and an explicit session, so the two
    are the *same kind of thing* — an isolated environment. Resolution
    (`clausters.base.main.Main.resolve_server`) duck-types on this: it reads the
    ambient environment's ``server`` whether that is the default session or a
    named one.
    """

    def __init__(self):
        super().__init__()
        #: the environment's server; ``None`` until one is set (the default
        #: session adopts one via a free-standing ``Server().boot()``; a `Session`
        #: is built around one).
        self.server = None
