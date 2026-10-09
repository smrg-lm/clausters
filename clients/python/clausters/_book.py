"""Where an installed package keeps its book, as Markdown.

The wheel carries the client's book in ``clausters/_docs/`` (staged by
``build_docs.py``): the same pages Read the Docs publishes, the API reference
included, read offline. `folder` names it; the package's
``clausters_editor.docs`` entry point points here, which is how
clausters-editor finds the book of the environment its session runs on.
"""

import os


def folder():
    """The book's folder, or ``None`` when this install carries none (a build
    with ``CLAUSTERS_SKIP_DOCS``, or a source checkout never built)."""
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_docs")
    return path if os.path.isdir(path) else None
