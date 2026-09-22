"""Every name a book imports from `clausters` exists.

Nothing else reads the code in a book. pyright reads the package, the tests and
the examples, and the doc build reads links, so a snippet whose import no longer
resolves compiles, tests and builds clean -- which is how the document chapter
came to import an `Arrangement` a rename had retired, and the GUI chapter a
`field` builder that had been removed. This does not run the snippets (most of
them need a server, a host or a file); it resolves every
``from clausters... import ...`` in a ```python block of the Python book, the
server book and the package's README, which is the half of a snippet a rename
breaks.

The web book has the same check in `clients/web/tests/book-imports.test.ts`.
"""

import importlib
import pathlib
import re

import pytest

PYTHON = pathlib.Path(__file__).resolve().parents[1]
REPO = PYTHON.parents[1]

# The server book's history and decision records quote code as it was, on
# purpose, so they are not read.
BOOKS = sorted(
    [*(PYTHON / "docs" / "src").rglob("*.md"), PYTHON / "README.md",
     *(p for p in (REPO / "docs").rglob("*.md")
       if "history" not in p.parts and p.name != "decisions.md")]
)

FENCE = re.compile(r"^```python[^\n]*\n(.*?)^```", re.M | re.S)
IMPORT = re.compile(
    r"^\s*from\s+(clausters(?:\.\w+)*)\s+import\s+(?:\(([^)]*)\)|([^\n#]+))", re.M)


def _imports():
    for path in BOOKS:
        text = path.read_text(encoding="utf-8")
        for block in FENCE.finditer(text):
            line0 = text.count("\n", 0, block.start(1)) + 1
            for m in IMPORT.finditer(block.group(1)):
                names = (m.group(2) or m.group(3)).replace("\\", " ")
                line = line0 + block.group(1).count("\n", 0, m.start())
                for name in names.replace("\n", " ").split(","):
                    name = name.split(" as ")[0].strip()
                    if name.isidentifier():
                        where = f"{path.relative_to(REPO)}:{line}"
                        yield pytest.param(m.group(1), name, id=f"{where} {m.group(1)}.{name}")


@pytest.mark.parametrize("module,name", list(_imports()))
def test_a_book_imports_what_exists(module, name):
    mod = importlib.import_module(module)
    if hasattr(mod, name):
        return
    # `from clausters import seq` names a submodule the package may not bind
    # until it is imported.
    importlib.import_module(f"{module}.{name}")


def test_the_books_have_imports_to_check():
    # A pattern that stopped matching would pass every book silently.
    assert len(list(_imports())) > 100
