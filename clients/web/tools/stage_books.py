"""Stage the books, as Markdown, into the package: the client's
(``dist/books/client/``) and the server's (``dist/books/server/``).

    python3 tools/stage_books.py    # from clients/web/; what build.sh runs

The npm package carries the client's book that Read the Docs publishes, so it
reads offline in clausters-editor's documentation viewer, which finds it
through the package's ``package.json`` (``"clausters-editor": {"docs": ...}``):
the hand-written pages of ``docs/src/`` and the API reference TypeDoc
generates from the TSDoc comments into ``docs/src/api/``. Beside it, the
server's book, since the package carries the engine: the same parts of it the
Python wheel carries.

This is the Python package's staging (``clients/python/build_docs.py``), not a
second one: `stage_book` copies the client's book with the same rules (the
table of contents headed by the book's title, the sections on getting the
package cut and links into them repointed online) and `server_stage` the
server's. Only what is the web book's own is here: its source, its title, the
section it cuts and where it is published.

TypeDoc is run here, as ``docs/build.sh`` runs it: ``TYPEDOC`` names the
binary when it is not ``typedoc`` on the PATH (see that script for the
install).
"""

import os
import subprocess
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # clients/web
SRC = os.path.join(HERE, "docs", "src")
OUT = os.path.join(HERE, "dist", "books", "client")
SERVER_OUT = os.path.join(HERE, "dist", "books", "server")
URL = "https://clausters-web.readthedocs.io/en/latest/"
TITLE = "clausters web client"

# The sections of the book that get the package, by page and heading; each goes
# with everything under it (building it from a checkout included).
CUT = {"getting-started.md": ("Get the package",)}

sys.path.insert(0, os.path.join(HERE, "..", "python"))
import build_docs  # noqa: E402 -- the Python package's staging, found beside it


def stage() -> None:
    typedoc = os.environ.get("TYPEDOC", "typedoc")
    try:
        subprocess.run([typedoc], cwd=HERE, check=True)
    except (OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(
            f"clausters: the API reference could not be generated ({error}). It "
            "needs TypeDoc -- see docs/build.sh for the install, or TYPEDOC for "
            "where it is."
        )
    count = build_docs.stage_book(SRC, OUT, TITLE, CUT, URL)
    print(f"clausters: staged the offline book ({count} pages) into dist/books/client/")
    if not build_docs.server_stage(SERVER_OUT):
        raise SystemExit("clausters: the server's book was not found beside this package")


if __name__ == "__main__":
    stage()
