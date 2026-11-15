"""The chapters both client books share, written once.

    an mdBook preprocessor: each client book's book.toml runs `run` with its
    own name; build_docs.stage_book uses `expand` and `render` for the copy a
    package carries

The two clients are one client in two languages, so most of their books is
one text. That text lives here, in ``src/``, and each book's chapter of the
same name is one line that includes it (``{{#include ../../../docs/src/x.md}}``).
This module turns the shared text into the book's own:

- **Code by language.** Every example is written twice, a Python block and a
  TypeScript one (``python``; ``ts``, ``js`` or ``html``), next to each other:
  a run of code blocks with nothing but blank lines between them is one
  example, and each book keeps its own language's blocks of it. A run that
  has one language and not the other stops the build. A block in any other
  language (``sh``, ``text``, ``toml``) is the same in both books.
- **Platform prose by marker.** ``<!-- python -->`` ... ``<!-- /python -->``
  and ``<!-- web -->`` ... ``<!-- /web -->`` hold what only one book says, a
  paragraph or a few words. Code inside one of them is that book's alone and
  needs no pair. Markers do not nest.
- **Short alternatives inline.** ``{{alt A || B}}`` is ``A`` in the Python
  book and ``B`` in the web one: a call written with keyword arguments in one
  language and an options object in the other, or a link to a page each book
  names differently.
- **Names by table.** ``{{api buffer_alloc}}`` is an API name spelled as each
  client spells it: as written for Python, and for the web through
  ``names.toml``, else by turning each part from snake_case to camelCase
  (``Server.sample_timebase`` -> ``Server.sampleTimebase``). Where the web
  book's API reference has been generated (``docs/src/api/``, by TypeDoc),
  every part of a name spelled for it has to be a word the reference has, so
  a spelling the rule gets wrong stops the build until the table has it.

The web book also refuses a backticked snake_case name in the shared prose
that the Python reference documents (``sample_timebase``): it is a Python
spelling that reached the web book, written where a ``{{api ...}}`` was meant.
"""

import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "src")
NAMES = os.path.join(HERE, "names.toml")
CLIENTS = os.path.dirname(HERE)
WEB_API = os.path.join(CLIENTS, "web", "docs", "src", "api")
PYTHON_TABLE = os.path.join(CLIENTS, "python", "docs", "api-names.json")

BOOKS = ("python", "web")
LANGUAGES = {
    "python": {"python", "py", "pycon"},
    "web": {"ts", "typescript", "js", "javascript", "html"},
}

_FENCE = re.compile(r"^(\s*)(```+|~~~+)\s*([\w+-]*)")
_REGION = re.compile(r"<!-- (python|web) -->(.*?)<!-- /\1 -->", re.S)
_REGION_LINE = re.compile(r"^[ \t]*<!-- (python|web) -->[ \t]*\n(.*?)^[ \t]*<!-- /\1 -->[ \t]*\n", re.S | re.M)
_MARKER = re.compile(r"<!-- /?(python|web) -->")
_ALT = re.compile(r"\{\{alt (.*?) \|\| (.*?)\}\}", re.S)
_API = re.compile(r"\{\{api ([A-Za-z_][A-Za-z0-9_.]*)\}\}")
_INCLUDE = re.compile(r"\{\{#include ([^}\s]+)\}\}")
_WORD = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")
_SNAKE = re.compile(r"(?<![`\w])`([a-z][a-z0-9]*(?:_[a-z0-9]+)+)(?:\(\))?`")


class BookError(Exception):
    """A shared chapter that cannot become a book's: the file and why."""


def _read_names():
    try:
        import tomllib
    except ImportError:  # Python < 3.11: the table is flat `a = "b"` lines
        tomllib = None
    try:
        with open(NAMES, "rb") as f:
            data = f.read()
    except OSError:
        return {}
    if tomllib is not None:
        return tomllib.loads(data.decode("utf-8")).get("web", {})
    found = {}
    for line in data.decode("utf-8").splitlines():
        m = re.match(r'\s*"?([\w.]+)"?\s*=\s*"([^"]*)"', line)
        if m:
            found[m.group(1)] = m.group(2)
    return found


def camel(name: str) -> str:
    """Each dotted part of ``name`` from snake_case to camelCase; a leading
    or doubled underscore is kept, a part with none is left as it is."""
    def part(p: str) -> str:
        head, *tail = p.split("_")
        if not head or any(not t for t in tail):
            return p
        return head + "".join(t[:1].upper() + t[1:] for t in tail)

    return ".".join(part(p) for p in name.split("."))


def _web_known():
    """Every word the web reference has, by itself: the headings TypeDoc
    writes for each class, member, function and parameter, and the option
    names it lays out inside a signature. None when the reference has not
    been generated."""
    if not os.path.isdir(WEB_API):
        return None
    known = set()
    for root, _, files in os.walk(WEB_API):
        for name in files:
            if name.endswith(".md"):
                with open(os.path.join(root, name), encoding="utf-8") as f:
                    known.update(_WORD.findall(f.read()))
    return known


def _python_known():
    try:
        with open(PYTHON_TABLE, encoding="utf-8") as f:
            table = json.load(f)
    except OSError:
        return set()
    return {ident.rsplit(".", 1)[-1] for ident in table["ids"]}


class Book:
    """One client book's reading of the shared text."""

    def __init__(self, book: str) -> None:
        if book not in BOOKS:
            raise ValueError(f"no client book is called {book!r}")
        self.book = book
        self.other = next(b for b in BOOKS if b != book)
        self.names = _read_names()
        self.web_known = _web_known() if book == "web" else None
        self.python_known = _python_known() if book == "web" else set()

    def spell(self, name: str, where: str) -> str:
        if self.book == "python":
            return name
        spelled = self.names.get(name) or camel(name)
        if self.web_known is not None:
            for part in spelled.split("."):
                if part not in self.web_known:
                    raise BookError(
                        f"{where}: {{{{api {name}}}}} is `{spelled}` in the web book, which its API "
                        f"reference does not document (`{part}`): give it its spelling in "
                        f"clients/docs/names.toml")
        return spelled

    def render(self, text: str, where: str) -> str:
        """``text``, a shared chapter, as this book reads it; ``where`` names
        it in an error."""
        check(text, where)
        keep, drop = self.book, self.other
        text = _REGION_LINE.sub(lambda m: m.group(2) if m.group(1) == keep else "", text)
        text = _REGION.sub(lambda m: m.group(2) if m.group(1) == keep else "", text)
        text = _ALT.sub(lambda m: m.group(1) if keep == "python" else m.group(2), text)
        text = _API.sub(lambda m: self.spell(m.group(1), where), text)
        text = _drop_blocks(text, LANGUAGES[drop])
        if self.book == "web":
            self._refuse_python_names(text, where)
        return text

    def _refuse_python_names(self, text: str, where: str) -> None:
        for number, line in _prose_lines(text):
            for m in _SNAKE.finditer(line):
                name = m.group(1)
                if name in self.python_known and not (self.web_known and name in self.web_known):
                    raise BookError(
                        f"{where}:{number}: `{name}` is the Python client's spelling, in the web "
                        f"book: write {{{{api {name}}}}}, or put it in a <!-- python --> region")


def _prose_lines(text: str):
    """The lines of ``text`` outside its code blocks, numbered."""
    fence = None
    for number, line in enumerate(text.split("\n"), 1):
        m = _FENCE.match(line)
        if m:
            if fence is None:
                fence = m.group(2)
            elif m.group(2).startswith(fence[0]) and len(m.group(2)) >= len(fence):
                fence = None
            continue
        if fence is None:
            yield number, line


def _blocks(text: str):
    """The fenced code blocks of ``text``: (first line, last line, language),
    by line index."""
    lines = text.split("\n")
    out, start, fence, lang = [], None, None, ""
    for i, line in enumerate(lines):
        m = _FENCE.match(line)
        if not m:
            continue
        if fence is None:
            start, fence, lang = i, m.group(2), m.group(3).lower()
        elif m.group(2)[0] == fence[0] and len(m.group(2)) >= len(fence) and not m.group(3):
            out.append((start, i, lang))
            fence = None
    if fence is not None:
        raise BookError(f"a code block opened on line {start + 1} is never closed")
    return lines, out


def _drop_blocks(text: str, languages) -> str:
    """``text`` without its code blocks in ``languages``, and the blank line
    that followed each."""
    lines, blocks = _blocks(text)
    gone = set()
    for start, end, lang in blocks:
        if lang in languages:
            gone.update(range(start, end + 1))
            if end + 1 < len(lines) and not lines[end + 1].strip():
                gone.add(end + 1)
    return "\n".join(line for i, line in enumerate(lines) if i not in gone)


def check(text: str, where: str) -> None:
    """Refuses a shared chapter whose markers do not close, or whose shared
    examples are not in both languages."""
    unmarked = _REGION.sub(lambda m: _blank(m.group(0)), text)
    stray = _MARKER.search(unmarked)
    if stray:
        line = unmarked.count("\n", 0, stray.start()) + 1
        raise BookError(f"{where}:{line}: {stray.group(0)} has no partner (markers do not nest)")
    for m in _REGION.finditer(text):
        try:
            _, blocks = _blocks(m.group(2))
        except BookError as error:
            raise BookError(f"{where}: {error}") from None
        foreign = [lang for _, _, lang in blocks if lang in LANGUAGES[next(b for b in BOOKS if b != m.group(1))]]
        if foreign:
            line = text.count("\n", 0, m.start()) + 1
            raise BookError(f"{where}:{line}: a {foreign[0]} block inside a <!-- {m.group(1)} --> region")
    try:
        lines, blocks = _blocks(unmarked)
    except BookError as error:
        raise BookError(f"{where}: {error}") from None
    runs, last_end = [], None
    for start, end, lang in blocks:
        between = lines[last_end + 1:start] if last_end is not None else None
        if between is not None and all(not line.strip() for line in between):
            runs[-1].append((start, lang))
        else:
            runs.append([(start, lang)])
        last_end = end
    for run in runs:
        has = {book for book in BOOKS if any(lang in LANGUAGES[book] for _, lang in run)}
        if len(has) == 1:
            (alone,) = has
            missing = next(b for b in BOOKS if b != alone)
            raise BookError(
                f"{where}:{run[0][0] + 1}: this example is written for the {alone} client only; "
                f"write its {missing} block beside it, or mark it <!-- {alone} -->")


def _blank(region: str) -> str:
    """A region's text replaced by as many empty lines, so line numbers hold."""
    return "\n" * region.count("\n")


def expand(text: str, page_dir: str) -> "tuple[str, bool]":
    """``text`` with each ``{{#include}}`` of a shared chapter replaced by the
    chapter, the path read from ``page_dir``; and whether it had one."""
    found = False

    def include(m):
        nonlocal found
        path = os.path.abspath(os.path.join(page_dir, m.group(1)))
        if os.path.commonpath([path, SRC]) != SRC:
            return m.group(0)
        found = True
        with open(path, encoding="utf-8") as f:
            return f.read()

    return _INCLUDE.sub(include, text), found


def shared(path: str) -> bool:
    """Whether ``path`` (a chapter's ``source_path`` resolved) is shared."""
    return os.path.commonpath([os.path.abspath(path), SRC]) == SRC


def run(data, book: str) -> None:
    """mdBook's preprocessor step: ``data`` is its ``[context, book]``. mdBook
    has already put each shared chapter into the page that includes it, so a
    chapter is shared when its text came from ``src/``: the page's own source
    is one ``{{#include}}`` line into it."""
    context, mdbook = data
    reader = Book(book)
    src = os.path.join(context["root"], context["config"]["book"].get("src", "src"))

    def walk(items) -> None:
        for item in items:
            chapter = item.get("Chapter") if isinstance(item, dict) else None
            if not chapter:
                continue
            source = chapter.get("source_path")
            if source:
                try:
                    with open(os.path.join(src, source), encoding="utf-8") as f:
                        own = f.read()
                except OSError:
                    own = ""
                if any(shared(os.path.join(src, os.path.dirname(source), p)) for p in _INCLUDE.findall(own)):
                    try:
                        chapter["content"] = reader.render(chapter["content"], source)
                    except BookError as error:
                        print(f"shared_book: {error}", file=sys.stderr)
                        sys.exit(1)
            walk(chapter.get("sub_items", []))

    walk(mdbook.get("sections") or mdbook.get("items") or [])
    json.dump(mdbook, sys.stdout)


def main(book: str) -> None:
    """The command book.toml runs: answers mdBook's ``supports`` question, and
    otherwise preprocesses the book on stdin."""
    if sys.argv[1:2] == ["supports"]:
        sys.exit(0)
    run(json.load(sys.stdin), book)
