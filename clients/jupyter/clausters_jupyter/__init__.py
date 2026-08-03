"""The Clausters GUI in a Jupyter notebook cell.

Importing this package is the whole setup, and after it the verbs of the
ordinary `clausters` client draw where you are looking:

```python
import clausters_jupyter
from clausters.defs import sine

plot(sine(440) * 0.5)
scope(bus=0)
```

The import wires the default in-page session; call `notebook` yourself first to
choose the other backend or to size the canvases. It only auto-wires **inside
an IPython shell** — imported by a script or a test the package does nothing
until asked, since there is no cell to draw in and a global session installed
by an import would be a surprise.

Nothing of this lives in `clausters`, which keeps no IPython logic and gains no
display hooks. The pieces are a carrier (`clausters_jupyter.carrier`), the
routing that gives each window its own cell (`clausters_jupyter.bridge`), the
widget that carries bytes (`clausters_jupyter.widget`) and formatters
registered from outside the classes they display
(`clausters_jupyter.formatters`).
"""

from .bridge import Bridge
from .carrier import CommInterface, RoundTripInCell
from .journal import Journal
from .session import audio, current, notebook
from .widget import ClaustersWidget

__all__ = [
    "notebook",
    "audio",
    "current",
    "Bridge",
    "CommInterface",
    "ClaustersWidget",
    "Journal",
    "RoundTripInCell",
]


def _autowire():
    """Wire the default session when imported from a running IPython shell."""
    try:
        from IPython import get_ipython
    except ImportError:
        return
    if get_ipython() is not None:
        # Flagged as auto so an explicit `notebook("native")` in the same cell
        # can still replace it -- the wiring costs nothing until a window is
        # displayed, and silently keeping the default would hand the caller a
        # backend they did not ask for.
        notebook(_autowiring=True)


_autowire()
