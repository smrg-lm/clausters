"""Execution context (port of ``sc3/base/main.py``).

A small singleton holding what routines and clocks need to find each other. The
project rule is to **avoid global state** (see the memory
``evitar-estados-globales-clausters``): the server and clock are passed
explicitly, so RT and NRT can coexist in one script. The one piece of ambient
context that must exist — "which routine is running right now" — is
**thread-local**, so several `TempoClock` threads (and a live RT clock next to
an offline NRT render) never clobber each other's current thread.

The optional ``default_clock`` is convenience-only sugar (``None`` by default,
never required). Anything that can be passed explicitly, is.
"""

import random
import threading


class Main:
    def __init__(self):
        #: opt-in convenience default; never required (avoid global state).
        self.default_clock = None
        self._local = threading.local()
        self._rng = random.Random()
        self._seed = None

    @property
    def current_tt(self):
        """The routine being resumed on **this thread** (thread-local), set by
        the clock around each wake, so ``Server`` can read the running routine's
        exact logical beat. ``None`` outside a routine."""
        return getattr(self._local, "current_tt", None)

    @current_tt.setter
    def current_tt(self, value):
        self._local.current_tt = value

    def seed(self, value=None):
        """Seeds the context RNG (None reseeds from entropy). Returns the seed
        actually used so a session can be reproduced."""
        self._seed = value
        self._rng.seed(value)
        return value

    @property
    def rng(self) -> random.Random:
        return self._rng

    def elapsed_beats(self) -> float:
        return self.default_clock.beats() if self.default_clock else 0.0


# The process-wide context (its `current_tt` is per-thread).
main = Main()
