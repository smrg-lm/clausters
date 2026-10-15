"""Stage the books, as Markdown, into the package: the client's
(``clausters/_docs/``) and the server's (``clausters/_server_docs/``).

    python clients/python/build_docs.py    # what setup.py runs before packaging

The wheel carries the client's book that Read the Docs publishes, so it reads
offline: the hand-written pages of ``docs/src/`` and the API reference that
``docs/gen_api.py`` generates from the docstrings into ``docs/src/api/``.
Relative ``.md`` links and the ``<a id>`` anchors of the API pages work in any
Markdown viewer that follows links, so the pages are copied as they are, with
three changes:

- mdBook's table of contents, ``SUMMARY.md``, is headed by the book's title
  instead of mdBook's ``# Summary`` (`retitle`). A viewer that reads it as the
  book's structure names the book by that heading.
- The hand-written pages get the links to the API reference that mdBook's
  preprocessor gives the published book (``docs/api_links.py``), so a name is
  a link in both or in neither.
- The sections on building and installing are cut (`CUT`). Whoever reads the
  copy inside an installed package has already installed it.

The server's book travels with the server binary the wheel bundles, so the
same package documents both. Only the parts a user reads are staged
(`SERVER_PARTS`, less `SERVER_LEFT_OUT`); the developer guide stays in the
repository and online. In both books, a link to a page or a section left out
is rewritten to its published address.

The API pages need ``pydoc-markdown`` (see ``docs/build.sh``), so building the
package does too. ``CLAUSTERS_SKIP_DOCS=1`` builds without the books, and
``CLAUSTERS_REQUIRE_COMPLETE`` refuses that skip, as it refuses the others.

``clausters._book`` is how an installed package names the folders; the
``clausters_editor.docs`` entry points point at it.
"""

import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "docs", "src")
OUT = os.path.join(HERE, "clausters", "_docs")
URL = "https://clausters-python.readthedocs.io/en/latest/"

# The server's book: the repository's root docs/, and where it is published.
SERVER_SRC = os.path.join(HERE, "..", "..", "docs")
SERVER_OUT = os.path.join(HERE, "clausters", "_server_docs")
SERVER_URL = "https://clausters.readthedocs.io/en/latest/"
# A page of docs/ the book does not list is not published: it is read in the
# repository.
SERVER_REPO = "https://github.com/smrg-lm/clausters/blob/main/docs/"

# The books' titles, from each book.toml, as the staged tables of contents
# name them.
TITLE = "clausters Python client"
SERVER_TITLE = "clausters server"

# The sections of the client's book that build or install the package, by page
# and heading. Each goes with everything under it, up to the next heading of
# its level.
CUT = {"getting-started.md": ("Install", "Run the tests")}

# The parts of the server's book a user reads, by their headings in its
# SUMMARY.md; the chapters ahead of the first part (the introduction) go too.
SERVER_PARTS = ("User Guide", "Library & Embedding")
# Pages of those parts that are about building the server from the repository.
SERVER_LEFT_OUT = ("getting-started.md",)

_LINK = re.compile(r"\]\(([^()\s#:]+\.md)(#[^)\s]*)?\)")


def retitle(summary: str, title: str) -> str:
    """``SUMMARY.md`` headed by the book's ``title`` rather than mdBook's
    ``# Summary``, which names nothing."""
    lines = summary.splitlines()
    if lines and lines[0].strip().lower() == "# summary":
        lines[0] = f"# {title}"
    return "\n".join(lines) + "\n"


def slug(heading: str) -> str:
    """A heading's anchor, as mdBook writes it."""
    text = re.sub(r"[^\w\s-]", "", heading.strip().lower())
    return re.sub(r"\s", "-", text)


def cut(text: str, headings: "tuple[str, ...]") -> "tuple[str, set[str]]":
    """``text`` without the sections under ``headings``, and the anchors of
    every heading that went with them. A heading that is not there stops the
    build: the page was edited, and the cut has to follow it."""
    lines = text.splitlines(keepends=True)
    out, anchors, found = [], set(), set()
    fence, cutting = False, None
    for line in lines:
        if line.lstrip().startswith(("```", "~~~")):
            fence = not fence
        m = None if fence and not line.lstrip().startswith(("```", "~~~")) else re.match(r"(#+) (.*)", line)
        if m:
            level, title = len(m.group(1)), m.group(2).strip()
            if cutting is not None and level <= cutting:
                cutting = None
            if cutting is None and title in headings:
                cutting = level
                found.add(title)
            if cutting is not None:
                anchors.add(slug(title))
        if cutting is None:
            out.append(line)
    missing = set(headings) - found
    if missing:
        raise SystemExit(f"clausters: build_docs.CUT names sections that are gone: {sorted(missing)}")
    return "".join(out), anchors


def repoint_sections(text: str, page: str, anchors: "set[str]", url: str) -> str:
    """``text`` with each link into a section cut from ``page`` pointing at
    that section online."""

    def repoint(m):
        target, anchor = m.group(1), (m.group(2) or "")[1:]
        if os.path.basename(target) == page and anchor in anchors:
            return f"]({url}{page[:-3]}.html#{anchor})"
        return m.group(0)

    return _LINK.sub(repoint, text)


def server_summary(summary: str) -> "tuple[str, list[str], list[str]]":
    """The server's ``SUMMARY.md`` cut to `SERVER_PARTS`, the pages it keeps
    and every page the whole book lists (the published ones)."""
    lines, pages, listed, keep = [], [], [], True
    for line in summary.splitlines()[1:]:
        if line.startswith("# "):
            keep = line[2:].strip() in SERVER_PARTS
        m = re.search(r"\]\(([^)]+\.md)\)", line)
        if m:
            listed.append(m.group(1))
        if not keep or (m and m.group(1) in SERVER_LEFT_OUT):
            continue
        lines.append(line)
        if m:
            pages.append(m.group(1))
    return retitle("# Summary\n" + "\n".join(lines), SERVER_TITLE), pages, listed


def server_page(text: str, pages: "list[str]", listed: "list[str]") -> str:
    """A staged page of the server's book, with each link to a page left out
    pointing at that page online: in the published book, or in the repository
    when the book does not list it."""

    def repoint(m):
        page, anchor = m.group(1), m.group(2) or ""
        if page in pages:
            return m.group(0)
        if page in listed:
            return f"]({SERVER_URL}{page[:-3]}.html{anchor})"
        return f"]({SERVER_REPO}{page}{anchor})"

    return _LINK.sub(repoint, text)


def server_stage() -> bool:
    """Copy the user's parts of the server's book into the package. Returns
    whether it was staged: outside the repository (an sdist) there is none."""
    shutil.rmtree(SERVER_OUT, ignore_errors=True)
    summary_path = os.path.join(SERVER_SRC, "SUMMARY.md")
    if not os.path.exists(summary_path):
        import build_native

        if os.environ.get(build_native._REQUIRE_COMPLETE):
            raise SystemExit(
                "clausters: the server's book was not found (docs/SUMMARY.md at "
                "the repository's root), and this build must leave nothing out.")
        print("clausters: no server book beside this package; the wheel carries the client's only")
        return False
    with open(summary_path, encoding="utf-8") as f:
        summary, pages, listed = server_summary(f.read())
    os.makedirs(SERVER_OUT)
    with open(os.path.join(SERVER_OUT, "SUMMARY.md"), "w", encoding="utf-8") as f:
        f.write(summary)
    for page in pages:
        with open(os.path.join(SERVER_SRC, page), encoding="utf-8") as f:
            text = f.read()
        out = os.path.join(SERVER_OUT, page)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        with open(out, "w", encoding="utf-8") as f:
            f.write(server_page(text, pages, listed))
    print(f"clausters: staged the server's book ({len(pages)} pages) into clausters/_server_docs/")
    return True


def stage() -> bool:
    """Generate the API pages and copy the book into the package. Returns
    whether a book was staged."""
    sys.path.insert(0, HERE)
    import build_native

    if build_native._skipping("CLAUSTERS_SKIP_DOCS", "the package carries no offline book"):
        shutil.rmtree(OUT, ignore_errors=True)
        shutil.rmtree(SERVER_OUT, ignore_errors=True)
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
    anchors = {}
    for page, headings in CUT.items():
        path = os.path.join(OUT, page)
        with open(path, encoding="utf-8") as f:
            text, anchors[page] = cut(f.read(), headings)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
    for root, _, files in os.walk(OUT):
        for name in files:
            if name.endswith(".md"):
                path = os.path.join(root, name)
                with open(path, encoding="utf-8") as f:
                    text = original = f.read()
                for page, cut_anchors in anchors.items():
                    text = repoint_sections(text, page, cut_anchors, URL)
                if text != original:
                    with open(path, "w", encoding="utf-8") as f:
                        f.write(text)
    with open(os.path.join(SRC, "SUMMARY.md"), encoding="utf-8") as f:
        summary = f.read()
    with open(os.path.join(OUT, "SUMMARY.md"), "w", encoding="utf-8") as f:
        f.write(retitle(summary, TITLE))
    count = sum(len([n for n in files if n.endswith(".md")]) for _, _, files in os.walk(OUT)) - 1
    print(f"clausters: staged the offline book ({count} pages) into clausters/_docs/")
    server_stage()
    return True


if __name__ == "__main__":
    stage()
