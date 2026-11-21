"""Triggers and control flow.

A **trigger** is a signal crossing from <= 0 up to > 0 -- one definition shared
by every function here, so the same crossing means the same thing whatever
produced it: `demand`, `send_trig` and the rest read it the same way. The
kinds whose only inputs are triggers default to control rate on the server,
because a trigger is the only thing that can move them.
"""

from ._generated.trig import *  # noqa: F403 -- the builders that mirror the wire
