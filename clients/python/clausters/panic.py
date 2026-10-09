"""The free-standing ``panic`` -- the stop-everything key.

`panic` stops whatever the **session in force** plays: the session a ``with``
block or `clausters.Session.activate` put in force, the running routine's, else
the default session (`clausters.default_session`). It is that session's
`clausters.base.environment.Environment.panic`: its clocks emptied, its
timelines halted, its server silenced (transports stopped, queued bundles
dropped, every node freed) and every responder not marked ``permanent``
freed. Nothing is closed, so the next ``play`` sounds.

```python
from clausters import Session, panic, play
from clausters.defs import sine

session = Session.live()
with session:
    play(sine(880) * 0.1)   # sounds until it is freed
    panic()                 # the session in force: this one
session.panic()             # the same, naming the session
```
"""

from .base.main import main

__all__ = ["panic"]


def panic():
    """Stop everything the session in force plays -- that session's
    `clausters.base.environment.Environment.panic` -- and answer the session.

    The session in force is the one an ambient `clausters.play` would use:
    a ``with session:`` block's, the running routine's, else the default
    session."""
    session = main._ambient_session()
    return (session if session is not None else main).panic()
