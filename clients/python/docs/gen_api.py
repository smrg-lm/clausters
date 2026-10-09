"""Generate the API reference pages of the Python client book, one per module.

    python3 docs/gen_api.py            # writes docs/src/api/, checks SUMMARY.md
    python3 docs/gen_api.py --summary  # prints the SUMMARY.md block it expects

Every module of the package is documented, found by walking it: a module is
public unless a component of its dotted name starts with an underscore (the
private loaders and wire helpers -- `_native`, `_osclib`, `_libpath`, ... --
are excluded that way). A hand-written list went stale once, silently: the
modules added after it were missing from the reference with no error.

Each module gets `src/api/<dotted.name>.md`: its docstring, a contents list and
its members. A package's page also lists its modules and subpackages, linked;
a page the guide links to lists those guide pages back ("In the guide"); and a
name in backticks that resolves to one documented object is a link to it
(`Linker`), so a docstring's `boot` or `ServerOptions` reads as a reference.
The pages are git-ignored and rebuilt by `build.sh`. mdBook renders only the
pages `src/SUMMARY.md` names, so after generating, the script checks that the
API block of `SUMMARY.md` is exactly the tree it wrote, and fails with the
block to paste when it is not.

pydoc-markdown does the parsing and the rendering, driven as a library over
`../pydoc-markdown.yml`, so the package is parsed once. When the running Python
does not have it (a `uv tool` install lives in its own environment), the script
re-runs itself with the interpreter of the `pydoc-markdown` command.
"""

import io
import os
import re
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CLIENT = os.path.dirname(HERE)  # clients/python: pydoc-markdown.yml and clausters/
OUT = os.path.join(HERE, "src", "api")
SUMMARY = os.path.join(HERE, "src", "SUMMARY.md")
SUMMARY_HEAD = "- [API reference](api/clausters.md)"


def reexec_with_pydoc_markdown() -> None:
    command = shutil.which("pydoc-markdown")
    if command is None:
        sys.exit("gen_api.py: pydoc-markdown is not installed (see build.sh)")
    with open(command, "rb") as fp:
        shebang = fp.readline().decode().strip()
    if not shebang.startswith("#!"):
        sys.exit(f"gen_api.py: cannot find the Python that runs {command}")
    interpreter = shebang[2:].split()
    os.execv(interpreter[0], [*interpreter, os.path.abspath(__file__), *sys.argv[1:]])


def discover() -> list:
    """Every public module of the package, as dotted names."""
    names = []
    for root, dirs, files in os.walk(os.path.join(CLIENT, "clausters")):
        dirs[:] = sorted(d for d in dirs if not d.startswith("_"))
        rel = os.path.relpath(root, CLIENT).split(os.sep)
        for f in sorted(files):
            if not f.endswith(".py"):
                continue
            stem = f[:-3]
            if stem == "__init__":
                names.append(".".join(rel))
            elif not stem.startswith("_"):
                names.append(".".join(rel + [stem]))
    return names


def children(name: str, names: list) -> list:
    """The modules and subpackages directly under `name`: modules first."""
    direct = [n for n in names if n.rpartition(".")[0] == name]
    packages = {n for n in direct if any(m.startswith(n + ".") for m in names)}
    return [n for n in direct if n not in packages] + [n for n in direct if n in packages]


def summary_block(names: list) -> str:
    lines = [SUMMARY_HEAD]

    def walk(name: str, depth: int) -> None:
        for child in children(name, names):
            lines.append(f"{'  ' * depth}- [{child.rpartition('.')[2]}](api/{child}.md)")
            walk(child, depth + 1)

    walk("clausters", 1)
    return "\n".join(lines) + "\n"


def summary_of(docstring) -> str:
    """A docstring's first paragraph, on one line."""
    text = getattr(docstring, "content", docstring) or ""
    return " ".join(text.strip().split("\n\n")[0].split())


def member_ids(module) -> dict:
    """Every documented object of a module, by dotted id (the id its `<a id>`
    anchor carries), mapped to whether it is a member of the module itself
    (a class or a function defined at its top) rather than of a class."""
    ids = {}

    def walk(obj, prefix: str, top: bool) -> None:
        for member in getattr(obj, "members", None) or []:
            ident = f"{prefix}.{member.name}"
            ids[ident] = top
            walk(member, ident, False)

    walk(module, module.name, True)
    return ids


class Linker:
    """Turns a name in backticks into a link to its entry in the reference.

    A docstring names what it talks about in backticks -- `boot`, `Server`,
    `clausters.seq.Pbind` -- and the reference has an anchor for each documented
    object, so the name can be a link. It is one only when it resolves to exactly
    one entry, tried in order: a member of the class the text is in (`boot` in
    the `Server` section), a member of the module, a dotted name whose ending is
    unique (`Server.boot`, `clausters.defs.server.Server`), and a class whose
    name only one module defines. Anything else stays code: a parameter, a
    value, a name two modules define, a lowercase name from another module.
    """

    SPAN = re.compile(r"(?<![`\[])`([A-Za-z_][A-Za-z0-9_.]*)(\(\))?`(?![`\]])")
    ARG = re.compile(r"^(\s*- `[A-Za-z_][A-Za-z0-9_]*`)")

    def __init__(self, modules: list) -> None:
        self.modules = sorted((m.name for m in modules), key=len, reverse=True)
        self.module_set = set(self.modules)
        self.ids = {}
        for module in modules:
            self.ids.update(member_ids(module))
        self.ids.update({m.name: False for m in modules})
        self.by_last = {}
        for ident, top in self.ids.items():
            if top:
                self.by_last.setdefault(ident.rpartition(".")[2], []).append(ident)

    def module_of(self, ident: str) -> str:
        return next(m for m in self.modules if ident == m or ident.startswith(m + "."))

    def resolve(self, name: str, module: str, section: str):
        if name.startswith("_") or "._" in name:
            return None
        if "." not in name:
            scope = section
            while scope and scope != module:
                candidate = f"{scope}.{name}"
                if candidate in self.ids:
                    return candidate
                scope = scope.rpartition(".")[0]
            own = f"{module}.{name}"
            # A submodule is named by its bare name only in its package's own
            # docstring; in a member's text the same word is a parameter.
            if own in self.ids and (own not in self.module_set or section == module):
                return own
            # Beyond the module, only a class: a lowercase word (`select`,
            # `channels`) is a parameter or an attribute far more often than
            # the one function elsewhere that happens to share it.
            if not name[0].isupper():
                return None
            found = self.by_last.get(name, [])
            return found[0] if len(found) == 1 else None
        if name in self.ids:
            return name
        found = [i for i in self.ids if i.endswith("." + name)]
        return found[0] if len(found) == 1 else None

    def href(self, ident: str, module: str) -> str:
        page = self.module_of(ident)
        if ident == page:
            return f"{page}.md"
        return f"#{ident}" if page == module else f"{page}.md#{ident}"

    def link(self, text: str, module: str) -> str:
        """``text``, one module's page, with its names linked. Code blocks,
        headings, existing links and an argument list's own names are left."""
        out = []
        section = module
        fence = False
        for line in text.split("\n"):
            anchor = re.match(r'<a id="([^"]+)"></a>', line)
            if anchor:
                section = anchor.group(1)
            if line.lstrip().startswith("```"):
                fence = not fence
            if fence or line.startswith("#") or line.lstrip().startswith("```"):
                out.append(line)
                continue
            head = ""
            arg = self.ARG.match(line)
            if arg:
                head, line = arg.group(1), line[arg.end():]

            def sub(m):
                target = self.resolve(m.group(1), module, section)
                if target is None or target == section:
                    return m.group(0)
                return f"[{m.group(0)}]({self.href(target, module)})"

            out.append(head + self.SPAN.sub(sub, line))
        return "\n".join(out)


def guide_pages() -> dict:
    """The book's hand-written pages that link into the reference: for each
    module, the pages (path from the book's root, title) that link to its page."""
    found = {}
    src = os.path.dirname(OUT)
    for root, dirs, files in os.walk(src):
        dirs[:] = sorted(d for d in dirs if os.path.join(root, d) != OUT)
        for f in sorted(files):
            if not f.endswith(".md") or f == "SUMMARY.md":
                continue
            path = os.path.join(root, f)
            rel = os.path.relpath(path, src).replace(os.sep, "/")
            with open(path, encoding="utf-8") as fp:
                text = fp.read()
            title = next((line[2:].strip() for line in text.splitlines() if line.startswith("# ")), rel)
            for module in set(re.findall(r"api/([A-Za-z0-9_.]+)\.md", text)):
                found.setdefault(module, []).append((rel, title))
    return found


def page(renderer, module, names: list, docs: dict, guides: list) -> str:
    """One module's page: its header and docstring, the list of what is under
    it when it is a package, the contents list, then the members."""
    body = renderer.render_to_string([module])
    under = children(module.name, names)
    parts = []
    if under:
        parts.append("**Modules**\n\n")
        for child in under:
            summary = summary_of(docs.get(child))
            name = child.rpartition(".")[2]
            parts.append(f"- [`{name}`]({child}.md)" + (f" -- {summary}" if summary else "") + "\n")
        parts.append("\n")
    if guides:
        parts.append("**In the guide**\n\n")
        for rel, title in sorted(guides, key=lambda g: g[1].lower()):
            parts.append(f"- [{title}](../{rel})\n")
        parts.append("\n")
    if module.members:
        toc = io.StringIO()
        for member in module.members:
            renderer._render_toc(toc, 0, member)
        parts.append("**Contents**\n\n" + toc.getvalue() + "\n")
    # After the module's own docstring, before its first member.
    cut = body.find(f'<a id="{module.name}.')
    if cut < 0:
        return body.rstrip("\n") + "\n\n" + "".join(parts)
    return body[:cut] + "".join(parts) + body[cut:]


def main() -> int:
    names = discover()
    if "--summary" in sys.argv[1:]:
        sys.stdout.write(summary_block(names))
        return 0

    try:
        import docspec
        from pydoc_markdown import PydocMarkdown
        from pydoc_markdown.interfaces import Context
    except ImportError:
        reexec_with_pydoc_markdown()

    os.chdir(CLIENT)
    config = PydocMarkdown()
    config.load_config("pydoc-markdown.yml")
    config.init(Context(directory=CLIENT))
    config.loaders[0].modules = names
    modules = config.load_modules()
    docs = {m.name: m.docstring for m in modules}
    config.process(modules)
    renderer = config.renderer
    renderer.process(modules, config.resolver)

    os.makedirs(OUT, exist_ok=True)
    for f in os.listdir(OUT):
        if f.endswith(".md"):
            os.remove(os.path.join(OUT, f))
    linker = Linker(modules)
    guides = guide_pages()
    for module in modules:
        assert isinstance(module, docspec.Module)
        text = page(renderer, module, names, docs, guides.get(module.name, []))
        with open(os.path.join(OUT, f"{module.name}.md"), "w", encoding="utf-8") as fp:
            fp.write(linker.link(text, module.name))

    expected = summary_block(names)
    with open(SUMMARY, encoding="utf-8") as fp:
        summary = fp.read()
    start = summary.find(SUMMARY_HEAD)
    end = start
    while start >= 0:
        nl = summary.find("\n", end)
        end = len(summary) if nl < 0 else nl + 1
        if not summary.startswith(" ", end):
            break
    if start < 0 or summary[start:end] != expected:
        sys.stderr.write(
            "gen_api.py: the API block of docs/src/SUMMARY.md does not match the modules.\n"
            "Replace it with (python3 docs/gen_api.py --summary):\n\n" + expected
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
