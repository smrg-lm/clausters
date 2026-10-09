"""Names in backticks become links to their entry in the API reference.

    an mdBook preprocessor: book.toml runs `run`, gen_api.py and build_docs.py
    use `Linker`

A page names what it talks about in backticks -- `boot`, `Server`,
`clausters.seq.Pbind`, `TempoClock.unlock` -- and the reference has an anchor
for every documented object, so the name can be a link. `gen_api.py` writes the
table of those objects (`api-names.json`, beside this file) when it generates
the reference, links the reference's own pages with it, and this module links
the hand-written pages: as mdBook's preprocessor for the published book, and
from `build_docs.py` for the copy the wheel carries. One table and one rule,
so a name is a link in both or in neither.

A name is a link only when it resolves to exactly one entry. In a reference
page, which knows the class or module its text belongs to, it is tried in
order as a member of that class (or of a class it derives from), a member of
the module, the one member of the module so named, a dotted name, and a class
only one module defines. In a guide page only the last two: a dotted name
(`Server.boot`, or a re-exported path like `clausters.base.TempoClock`) and a
capitalized class name. Anything else stays code: a parameter, a value, a word
two modules define, a lowercase name with no context to settle it.
"""

import json
import os
import posixpath
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TABLE = os.path.join(HERE, "api-names.json")


def table_of(modules) -> dict:
    """The table from pydoc-markdown's parsed modules: every documented object
    by dotted id, whether it is a module's own member, and each class's bases."""
    ids, bases = {}, {}

    def walk(obj, prefix: str, top: bool) -> None:
        for member in getattr(obj, "members", None) or []:
            ident = f"{prefix}.{member.name}"
            ids[ident] = top
            if getattr(member, "bases", None) is not None:
                bases[ident] = [str(b) for b in member.bases]
            walk(member, ident, False)

    for module in modules:
        walk(module, module.name, True)
    return {"modules": [m.name for m in modules], "ids": ids, "bases": bases}


class Linker:
    SPAN = re.compile(r"(?<![`\[])`([A-Za-z_][A-Za-z0-9_.]*)(\(\))?`(?![`\]])")
    ARG = re.compile(r"^(\s*- `[A-Za-z_][A-Za-z0-9_]*`)")
    ANCHOR = re.compile(r'<a id="([^"]+)"></a>')

    def __init__(self, table: dict) -> None:
        self.modules = sorted(table["modules"], key=len, reverse=True)
        self.module_set = set(self.modules)
        self.ids = dict(table["ids"])
        self.ids.update({m: False for m in self.modules})
        self.by_last = {}
        for ident, top in self.ids.items():
            if top:
                self.by_last.setdefault(ident.rpartition(".")[2], []).append(ident)
        self.bases = {
            cls: [b for b in (self.dotted(name) or self.unique_class(name) for name in names) if b]
            for cls, names in table["bases"].items()
        }

    def module_of(self, ident: str) -> str:
        return next(m for m in self.modules if ident == m or ident.startswith(m + "."))

    def unique_class(self, name: str):
        found = [i for i in self.by_last.get(name.rpartition(".")[2], []) if i[0].isalpha()]
        return found[0] if len(found) == 1 else None

    def dotted(self, name: str):
        """A dotted name: the id itself, else the longest ending of it that only
        one id ends with, when the parts left over are a package that id lives
        under -- a path through a re-export, `clausters.base.TempoClock`."""
        if name in self.ids:
            return name
        parts = name.split(".")
        for k in range(len(parts), 0, -1):
            tail = "." + ".".join(parts[-k:])
            found = [i for i in self.ids if i.endswith(tail)]
            # The parts left over have to be where the id lives: a package
            # re-exports what its submodules define, never the other way.
            if len(found) == 1 and found[0].startswith(".".join(parts[:-k] or parts[:1])):
                return found[0]
            if found:
                return None
        return None

    def in_class(self, cls: str, name: str, seen=()):
        if f"{cls}.{name}" in self.ids:
            return f"{cls}.{name}"
        for base in self.bases.get(cls, []):
            if base not in seen:
                found = self.in_class(base, name, seen + (cls,))
                if found:
                    return found
        return None

    def resolve(self, name: str, module, section):
        if name.startswith("_") or "._" in name:
            return None
        if "." in name:
            return self.dotted(name)
        if module is not None:
            scope = section
            while scope and scope != module:
                found = self.in_class(scope, name)
                if found:
                    return found
                scope = scope.rpartition(".")[0]
            own = f"{module}.{name}"
            # A submodule is named by its bare name only in its package's own
            # docstring; in a member's text the same word is a parameter.
            if own in self.ids and (own not in self.module_set or section == module):
                return own
            inside = [i for i in self.ids
                      if i.startswith(module + ".") and i.endswith("." + name)
                      and self.module_of(i) == module]
            if len(inside) == 1:
                return inside[0]
        # Beyond its own module, only a class: a lowercase word (`select`,
        # `channels`) is a parameter far more often than the one function
        # elsewhere that happens to share it.
        if not name[0].isupper():
            return None
        found = self.by_last.get(name, [])
        return found[0] if len(found) == 1 else None

    def link(self, text: str, page: str, module=None) -> str:
        """``text``, the page at ``page`` (its path in the book), with its names
        linked. ``module`` is the reference page's module, or None for a guide.
        Code blocks, headings, existing links and an argument list's own names
        are left as they are."""
        out = []
        section = module
        fence = False
        here = posixpath.dirname(page)
        for line in text.split("\n"):
            anchor = self.ANCHOR.match(line)
            if anchor:
                section = anchor.group(1)
            if line.lstrip().startswith(("```", "~~~")):
                fence = not fence
                out.append(line)
                continue
            if fence or line.startswith("#"):
                out.append(line)
                continue
            head = ""
            arg = self.ARG.match(line) if module else None
            if arg:
                head, line = arg.group(1), line[arg.end():]

            def sub(m):
                target = self.resolve(m.group(1), module, section)
                if target is None or target == section:
                    return m.group(0)
                target_page = f"api/{self.module_of(target)}.md"
                fragment = "" if target in self.module_set else f"#{target}"
                if target_page == page:
                    href = fragment or "#"
                else:
                    href = posixpath.relpath(target_page, here or ".") + fragment
                return f"[{m.group(0)}]({href})"

            out.append(head + self.SPAN.sub(sub, line))
        return "\n".join(out)


def load():
    """The linker over the table gen_api.py wrote, or None before it ran."""
    try:
        with open(TABLE, encoding="utf-8") as f:
            return Linker(json.load(f))
    except OSError:
        return None


def run(data) -> None:
    """mdBook's preprocessor step: ``data`` is its ``[context, book]``, and the
    book goes back on stdout with the guide pages linked. book.toml's command
    reads it and imports this module from the book's root, which mdBook passes
    in the context: mdBook runs the command from wherever it was itself run."""
    _context, book = data
    linker = load()

    def walk(items) -> None:
        for item in items:
            chapter = item.get("Chapter") if isinstance(item, dict) else None
            if not chapter:
                continue
            path = chapter.get("path") or ""
            if linker and path and not path.startswith("api/"):
                chapter["content"] = linker.link(chapter["content"], path)
            walk(chapter.get("sub_items", []))

    walk(book.get("sections") or book.get("items") or [])
    json.dump(book, sys.stdout)


if __name__ == "__main__":
    if sys.argv[1:2] == ["supports"]:
        sys.exit(0)
    run(json.load(sys.stdin))
