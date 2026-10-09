"""Stage the client's book, as Markdown, into the package (``clausters/_docs/``).

    python clients/python/build_docs.py    # what setup.py runs before packaging

The wheel carries the same book Read the Docs publishes, so it reads offline:
the hand-written pages of ``docs/src/`` and the API reference that
``docs/gen_api.py`` generates from the docstrings into ``docs/src/api/``. They
are copied as they are -- relative ``.md`` links and the ``<a id>`` anchors of
the API pages work in any Markdown viewer that follows links -- with one
change: mdBook's table of contents, ``SUMMARY.md``, becomes ``index.md``, the
page a viewer opens first (`contents_page`), and the hand-written pages get the
links to the API reference that mdBook's preprocessor gives the published book
(``docs/api_links.py``), so a name is a link in both or in neither.

The API pages need ``pydoc-markdown`` (see ``docs/build.sh``), so building the
package does too. ``CLAUSTERS_SKIP_DOCS=1`` builds without the book, and
``CLAUSTERS_REQUIRE_COMPLETE`` refuses that skip, as it refuses the others.

``clausters._book.folder`` is how an installed package names the folder; the
``clausters_editor.docs`` entry point points at it.
"""

import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "docs", "src")
OUT = os.path.join(HERE, "clausters", "_docs")

# The book's title, from docs/book.toml, heads the staged contents page.
TITLE = "# clausters -- Python client"


def contents_page(summary: str) -> str:
    """mdBook's ``SUMMARY.md`` as a page to read: the book's title, each part
    title (``# User Guide``) a section under it, a chapter outside a part
    (``[Introduction](introduction.md)``) a list item like the rest, and the
    separators gone. mdBook reads those forms as its sidebar's structure; a
    reader sees them as a run of top-level headings."""
    lines = [TITLE]
    for line in summary.splitlines()[1:]:
        if line.startswith("# "):
            lines.append("#" + line)
        elif line.startswith("["):
            lines.append("- " + line)
        elif line.strip() != "---":
            lines.append(line)
    return "\n".join(lines) + "\n"


def stage() -> bool:
    """Generate the API pages and copy the book into the package. Returns
    whether a book was staged."""
    sys.path.insert(0, HERE)
    import build_native

    if build_native._skipping("CLAUSTERS_SKIP_DOCS", "the package carries no offline book"):
        shutil.rmtree(OUT, ignore_errors=True)
        return False
    gen = subprocess.run([sys.executable, os.path.join(HERE, "docs", "gen_api.py")])
    if gen.returncode != 0:
        raise SystemExit(
            "clausters: the API reference could not be generated (docs/gen_api.py, "
            "above). It needs pydoc-markdown -- `uv tool install --python 3.12 "
            "pydoc-markdown` -- or set CLAUSTERS_SKIP_DOCS=1 to build without the "
            "offline book."
        )
    shutil.rmtree(OUT, ignore_errors=True)
    shutil.copytree(SRC, OUT, ignore=shutil.ignore_patterns("SUMMARY.md"))
    sys.path.insert(0, os.path.join(HERE, "docs"))
    import api_links

    linker = api_links.load()
    for root, dirs, files in os.walk(OUT):
        dirs[:] = [d for d in dirs if os.path.join(root, d) != os.path.join(OUT, "api")]
        for name in files:
            if name.endswith(".md"):
                path = os.path.join(root, name)
                rel = os.path.relpath(path, OUT).replace(os.sep, "/")
                with open(path, encoding="utf-8") as f:
                    text = f.read()
                with open(path, "w", encoding="utf-8") as f:
                    f.write(linker.link(text, rel))
    with open(os.path.join(SRC, "SUMMARY.md"), encoding="utf-8") as f:
        summary = f.read()
    with open(os.path.join(OUT, "index.md"), "w", encoding="utf-8") as f:
        f.write(contents_page(summary))
    count = sum(len([n for n in files if n.endswith(".md")]) for _, _, files in os.walk(OUT))
    print(f"clausters: staged the offline book ({count} pages) into clausters/_docs/")
    return True


if __name__ == "__main__":
    stage()
