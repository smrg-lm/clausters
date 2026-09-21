"""Generate the API reference pages of the Python client book, one per module.

    python3 docs/gen_api.py            # writes docs/src/api/, checks SUMMARY.md
    python3 docs/gen_api.py --summary  # prints the SUMMARY.md block it expects

Every module of the package is documented, found by walking it: a module is
public unless a component of its dotted name starts with an underscore (the
private loaders and wire helpers -- `_native`, `_osclib`, `_libpath`, ... --
are excluded that way). A hand-written list went stale once, silently: the
modules added after it were missing from the reference with no error.

Each module gets `src/api/<dotted.name>.md`: its docstring, a contents list and
its members. A package's page also lists its modules and subpackages, linked.
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


def page(renderer, module, names: list, docs: dict) -> str:
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
    for module in modules:
        assert isinstance(module, docspec.Module)
        with open(os.path.join(OUT, f"{module.name}.md"), "w", encoding="utf-8") as fp:
            fp.write(page(renderer, module, names, docs))

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
