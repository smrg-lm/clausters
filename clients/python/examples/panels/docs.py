#!/usr/bin/env python3
"""A documentation panel: a small book, read as an editor reads its books.

A ``docs`` element is given the books it shows (``books``, each one's table
of contents) and their documents (``documents``, the Markdown of each by its
path), and draws them: a start page listing every book, a book's contents, a
document with its headings, lists, a table, a quote and a code block in the
colors of its syntax, with a button that copies it. Click a chapter to open
it, a link to go on to another document at its heading, and the arrows at the
top left to go back and forward; the strip says which book and chapter the
page is. **Ctrl+F** finds in the page (Enter for the next match) and
**Ctrl+Shift+F** searches every document.

A link out of the documentation is reported to this script (``"link"``),
which prints it; a program would open the browser there. Help on a name is
one set: ``docs.set(help={"name": ...})`` opens the section that is it.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/docs.py``. No audio server is
involved, so this boots only the GUI host. Needs a display and a GPU adapter.
"""

# %%
import sys

from clausters.gui import GuiHost, docs, view

# %% [markdown]
# ## The book
# Its table of contents, and the Markdown of its two documents.

# %%
BOOKS = [{
    "prefix": "",
    "title": "A small book",
    "parts": [{"title": None, "chapters": [
        {"title": "Getting started", "path": "start.md", "children": [
            {"title": "Buffers", "path": "buffers.md", "children": []},
        ]},
    ]}],
}]

DOCUMENTS = {
    "start.md": """# Getting started

A **session** boots a server and plays *what you write*. Read about
[buffers](buffers.md#reading) next, or visit [the site](https://example.org).

1. Boot it
2. Play a note
   - softly
   - loudly

| Verb | What it does |
|---|---|
| `boot` | starts the server |
| `quit` | stops it |

> A quote reads quieter than the text around it.
""",
    "buffers.md": """# Buffers

A buffer holds samples.

## Reading

Read a file into one:

```python
from clausters import Buffer

b = Buffer.read("a.wav")  # the whole file
print(b.frames)
```
""",
}

host = GuiHost().boot()
window = view(
    docs(books=BOOKS, documents=DOCUMENTS, name="docs"),
    title="docs", w=640, h=560,
).open(host=host)
panel = window.widget("docs")

# %% [markdown]
# ## A link out, and help on a name
# A link out of the book is the program's to open; help on a name opens the
# section that is it.

# %%
panel.on_event(lambda *payload: print("docs", *payload))
panel.set(help={"name": "Reading"})

# %% [markdown]
# ## Keep it open
# Close the window to end the run.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        window.wait()
    finally:
        print(panel.query().props)
        host.stop()
else:
    print("the panel is up - window.wait(10) to hold it, host.stop() to end")
