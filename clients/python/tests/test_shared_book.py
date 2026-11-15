"""The chapters both client books share become each book's own.

``clients/docs/shared_book.py`` is mdBook's preprocessor for both client books
and what the packages' staged copies go through; these pin its rules: each
book keeps its language's blocks and its platform's prose, an example in one
language alone stops the build, and a name is spelled per client.
"""

import pathlib
import sys

import pytest

REPO = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "clients" / "docs"))

import shared_book  # noqa: E402

PAIR = """Before.

```python
s = Server().boot()
```

```ts
const s = await new Server().boot();
```

After.
"""


def book(name):
    reader = shared_book.Book(name)
    reader.web_known = None  # the generated reference is not what these test
    reader.python_known = {"sample_timebase"}
    return reader


def test_each_book_keeps_its_own_language():
    python = book("python").render(PAIR, "x.md")
    web = book("web").render(PAIR, "x.md")
    assert "Server().boot()" in python and "await" not in python
    assert "await new Server().boot()" in web and "```python" not in web
    assert python.startswith("Before.") and python.rstrip().endswith("After.")


def test_an_example_in_one_language_stops_the_build():
    with pytest.raises(shared_book.BookError, match="written for the python client only"):
        book("web").render("```python\nx = 1\n```\n", "x.md")


def test_a_platform_region_needs_no_pair_and_is_dropped_by_the_other_book():
    text = "A.\n\n<!-- python -->\n```python\nx = 1\n```\n<!-- /python -->\n\nB <!-- web -->only here<!-- /web -->.\n"
    python = book("python").render(text, "x.md")
    web = book("web").render(text, "x.md")
    assert "x = 1" in python and "only here" not in python
    assert "x = 1" not in web and "B only here." in web


def test_a_foreign_block_in_a_region_is_refused():
    text = "<!-- python -->\n```ts\nlet x = 1;\n```\n<!-- /python -->\n"
    with pytest.raises(shared_book.BookError, match="inside a <!-- python --> region"):
        book("python").render(text, "x.md")


def test_a_marker_without_its_partner_is_refused():
    with pytest.raises(shared_book.BookError, match="has no partner"):
        book("python").render("A <!-- web -->never closed.\n", "x.md")


def test_alternatives_and_names_follow_the_book():
    text = "{{alt `f(x=1)` || `f({ x: 1 })`}} and `{{api Server.sample_timebase}}()`.\n"
    assert book("python").render(text, "x.md") == "`f(x=1)` and `Server.sample_timebase()`.\n"
    assert book("web").render(text, "x.md") == "`f({ x: 1 })` and `Server.sampleTimebase()`.\n"


def test_the_table_spells_what_the_rule_cannot():
    reader = book("web")
    reader.names = {"Server.sample_clock": "sampleClockFor"}
    assert reader.render("`{{api Server.sample_clock}}`\n", "x.md") == "`sampleClockFor`\n"


def test_a_spelling_the_web_reference_lacks_stops_the_build():
    reader = book("web")
    reader.web_known = {"Server"}
    with pytest.raises(shared_book.BookError, match="does not document"):
        reader.render("`{{api Server.boot_it}}`\n", "x.md")


def test_a_python_spelling_in_the_web_book_is_refused():
    with pytest.raises(shared_book.BookError, match="Python client's spelling"):
        book("web").render("Call `sample_timebase` first.\n", "x.md")
    assert "sample_timebase" in book("python").render("Call `sample_timebase` first.\n", "x.md")


def test_camel_keeps_what_has_no_underscore():
    assert shared_book.camel("Server.sample_timebase") == "Server.sampleTimebase"
    assert shared_book.camel("_private") == "_private"
    assert shared_book.camel("Session") == "Session"


def test_every_shared_chapter_reads_in_both_books():
    for path in sorted((REPO / "clients" / "docs" / "src").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        for name in shared_book.BOOKS:
            reader = shared_book.Book(name)
            reader.web_known = None
            reader.render(text, path.name)
