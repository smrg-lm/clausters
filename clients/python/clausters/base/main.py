"""Global execution context (port of ``sc3/base/main.py``).

A small singleton holding what routines and clocks need to find each other: the
default clock, the currently running time-thread (routine), and a seedable RNG
context for reproducibility. It is intentionally light in C2 — the full sc3
``Main`` (system/app clocks, status, server tree) grows in later milestones.
"""

import random


class Main:
    def __init__(self):
        self.default_clock = None
        # The routine currently being resumed by a clock (set by the clock
        # around each wake), so library code can find "the current thread".
        self.current_tt = None
        self._rng = random.Random()
        self._seed = None

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


# The process-wide context.
main = Main()
