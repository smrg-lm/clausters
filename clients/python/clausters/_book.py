"""Where an installed package keeps its books, as Markdown.

The wheel carries the client's book in ``clausters/_docs/`` (staged by
``build_docs.py``): the same pages Read the Docs publishes, the API reference
included, read offline. Beside it, ``clausters/_server_docs/`` holds the
user's parts of the server's book, since the wheel bundles the server too.
`folder` and `server` name them; the package's ``clausters_editor.docs`` entry
points point here, which is how clausters-editor finds the books of the
environment its session runs on.
"""

import os


def folder():
    """The book's folder, or ``None`` when this install carries none (a build
    with ``CLAUSTERS_SKIP_DOCS``, or a source checkout never built)."""
    return _staged("_docs")


def server():
    """The server's book's folder, or ``None`` when this install carries none
    (as `folder`, or a build from outside the repository)."""
    return _staged("_server_docs")


def _staged(name):
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), name)
    return path if os.path.isdir(path) else None
