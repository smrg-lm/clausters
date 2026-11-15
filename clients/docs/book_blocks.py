"""The client books' code blocks compile against their packages.

    python3 clients/docs/book_blocks.py web       (scripts/check-docs.sh web)
    python3 clients/docs/book_blocks.py python    (scripts/check-docs.sh python)

The Python book's blocks are read by the pyright the client is checked with,
with the client's own rules (clients/python/pyrightconfig.json) but for an
undefined name, which a fragment always has; that finds a module that does
not exist, a name it does not export, a call with an argument the function
does not take. The web book's are compiled, which says much more, as follows.

A book's example is a fragment: it uses a `server`, a `take` or a `seq` an
earlier block made, and the first line of a page is not repeated in every
block. So each block is compiled as a module of its own, and what it uses
without declaring is declared for it:

- a name the package exports is imported from the module the package exports
  it from, so it is checked with its real type -- a class always, a namespace
  (`gui`, `notation`) unless it is a word the books also use as a variable
  (`seq`, `data`, `multitrack`), a function only where every use is a call;
- a variable an earlier block made with `new Cls(...)` is declared as a
  `Cls`: from any section of the page when the page makes it once, and from
  the same section when it makes the name more than once;
- anything else is `any`, which checks nothing but stops nothing either.

What is left is what a reader who copies the block into a page that has the
names it assumes gets from the compiler: a method that does not exist, an
argument of the wrong shape, a `const` assigned twice, a property written
where the client has a setter. `book_blocks.toml` names each error that is
known and why it stands; anything else fails.

The package's own `tsc` does the compiling (`clients/web/node_modules`, from
`npm ci` there), through the package's `tsconfig.json`, against its sources
by the paths its `exports` map names, so no build is needed.
"""

import glob
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import shared_book  # noqa: E402

WEB = os.path.join(os.path.dirname(HERE), "web")
TSC = os.path.join(WEB, "node_modules", ".bin", "tsc")
KNOWN = os.path.join(HERE, "book_blocks.toml")
PYTHON = os.path.join(os.path.dirname(HERE), "python")
# The pyright CI checks the client with (.github/workflows/ci.yml).
PYRIGHT = "pyright@1.1.414"

FENCE = re.compile(r"^(\s*)```(ts|typescript|js|javascript)\s*$")
NEW = re.compile(r"^(?:export )?(?:const|let) (\w+)\s*=\s*(?:await )?new ([A-Z]\w*)\(", re.M)
DECLARED = re.compile(r"^(?:export )?(?:const|let) (\w+)\s*=", re.M)
ERROR = re.compile(r"^(b\d+\.ts)\((\d+),(\d+)\): error (TS\d+): (.*)$")
UNDECLARED = {"TS2304", "TS2552", "TS18004"}
# Words the books use for a variable that the package also exports as a
# namespace: a block that means the namespace imports it.
VARIABLES = {"seq", "data", "segments", "errors", "document", "multitrack"}
# Globals of the DOM or of node that a book's names would otherwise resolve
# to: the package's `Event` and `Buffer`, a variable called `window`.
SHADOWED = {"Event", "Buffer", "window", "process"}


def blocks():
    """Every TypeScript or JavaScript block the web book holds, as
    ``(chapter, line, code, inherited)``: the shared chapters as the web book
    reads them, and its own pages."""
    book = shared_book.Book("web")
    pages = [(p, book.render(open(p, encoding="utf-8").read(), p))
             for p in sorted(glob.glob(os.path.join(HERE, "src", "**", "*.md"), recursive=True))]
    for p in sorted(glob.glob(os.path.join(WEB, "docs", "src", "*.md"))):
        text = open(p, encoding="utf-8").read()
        if "{{#include" not in text:
            pages.append((p, text))
    for path, text in pages:
        lines = text.split("\n")
        # A name the page makes once is the same thing in every section; one
        # it makes more than once is only inherited inside a section.
        counts = {}
        for n in DECLARED.finditer(text):
            counts[n.group(1)] = counts.get(n.group(1), 0) + 1
        made = {}
        i = 0
        while i < len(lines):
            if lines[i].startswith("## "):
                made = {k: v for k, v in made.items() if counts.get(k) == 1}
            m = FENCE.match(lines[i])
            if m:
                indent, start = len(m.group(1)), i + 1
                end = start
                while not lines[end].strip().startswith("```"):
                    end += 1
                code = "\n".join(line[indent:] for line in lines[start:end])
                yield os.path.relpath(path, os.path.dirname(HERE)), start + 1, code, dict(made)
                # A name made again any other way is no longer the class an
                # earlier block made it as.
                for n in DECLARED.finditer(code):
                    made.pop(n.group(1), None)
                for n in NEW.finditer(code):
                    made[n.group(1)] = n.group(2)
                i = end
            i += 1


def module_of(code: str, made: dict) -> str:
    """``code`` as a module tsc can read: a `{ ... }` or `(...)` that elides
    something made empty, a fragment that returns wrapped in a function, and
    the variables an earlier block made declared with their class."""
    code = re.sub(r"\{\s*\.\.\.\s*\}", "({} as any)", code)
    code = re.sub(r"\[\s*\.\.\.\s*\]", "([] as any[])", code)
    code = re.sub(r"\(\s*\.\.\.\s*\)", "()", code)
    code = re.sub(r",\s*\.\.\.\s*(?=[)\]}])", "", code)
    if re.search(r"^return\b", code, re.M):
        lines = code.split("\n")
        imports = [line for line in lines if line.startswith("import ")]
        rest = [line for line in lines if not line.startswith("import ")]
        code = "\n".join(imports + ["async function fragment() {"] + rest + ["}"])
    head = []
    for var, cls in made.items():
        used = re.search(rf"(?<![\w.$]){var}\b", code)
        called = re.search(rf"(?<![\w.$]){var}\s*\(", code)
        if used and not called and not declares(code, var):
            head.append(f"declare let {var}: {cls};")
            head.append(f"//needs {cls}")
    return "\n".join(head + [code, "export {};", ""])


def declares(code: str, name: str) -> bool:
    n = re.escape(name)
    return re.search(rf"\b(const|let|var|function|class)\s+{n}\b"
                     rf"|\bimport\s*\{{[^}}]*\b{n}\b[^}}]*\}}"
                     rf"|\b(const|let|var)\s*[{{[][^=\]}}]*\b{n}\b[^=\]}}]*[\]}}]", code) is not None


def tsconfig(root: str, include: str) -> None:
    pkg = json.load(open(os.path.join(WEB, "package.json"), encoding="utf-8"))
    paths = {}
    for key, entry in pkg["exports"].items():
        target = entry["default"].replace("./dist/", "src/")
        paths["clausters" if key == "." else "clausters/" + key[2:]] = [
            os.path.join(WEB, re.sub(r"\.js$", ".ts", target))]
    json.dump({
        "extends": os.path.join(WEB, "tsconfig.json"),
        "compilerOptions": {
            "paths": paths,
            "typeRoots": [os.path.join(WEB, "node_modules", "@types")],
            "noImplicitAny": False,
            "allowImportingTsExtensions": True,
            "verbatimModuleSyntax": False,
        },
        # The worker library the engine's sources reference.
        "include": [include, os.path.join(WEB, "src", "engine", "nrt-worker.ts")],
    }, open(os.path.join(root, "tsconfig.json"), "w"))
    open(os.path.join(root, "package.json"), "w").write('{"type": "module"}\n')


def tsc(root: str) -> list:
    out = subprocess.run([TSC, "-p", ".", "--noErrorTruncation"], capture_output=True, text=True, cwd=root)
    return out.stdout.splitlines()


def exports(root: str) -> dict:
    """What each of the package's modules exports, by asking tsc: a probe
    that assigns a name nothing exports is told every name that would do."""
    probe = os.path.join(root, "probe")
    os.makedirs(probe)
    pkg = json.load(open(os.path.join(WEB, "package.json"), encoding="utf-8"))
    specs = ["clausters" if k == "." else "clausters/" + k[2:] for k in pkg["exports"]]
    for i, spec in enumerate(specs):
        open(os.path.join(probe, f"p{i}.ts"), "w").write(
            f'import * as M from "{spec}";\ntype V = typeof M;\n'
            'export const all: keyof V = "@@";\n'
            'export const fns: { [K in keyof V]: V[K] extends (...a: any) => any ? K : never }[keyof V] = "@@";\n'
            'export const classes: { [K in keyof V]: V[K] extends abstract new (...a: any) => any ? K : never }'
            '[keyof V] = "@@";\n')
    tsconfig(probe, "p*.ts")
    found = {}
    for line in tsc(probe):
        m = re.match(r"^p(\d+)\.ts\((\d+),\d+\): error TS2322: Type '\"@@\"' is not assignable to type '(.*)'\.$", line)
        if not m:
            continue
        spec, kind = specs[int(m.group(1))], {3: "value", 4: "function", 5: "class"}[int(m.group(2))]
        for name in re.findall(r'"([^"]+)"', m.group(3)):
            found.setdefault(name, {}).setdefault(spec, set()).add(kind)
    return found


def importable(name: str, code: str, names: dict):
    """The import a free ``name`` gets, or None for `any`."""
    where = names.get(name)
    if not where:
        return None
    spec = "clausters" if "clausters" in where else sorted(where)[0]
    kinds = where[spec]
    if len(where) > 1 and "clausters" not in where:
        return None  # two modules, two things: nothing says which
    uses = re.findall(rf"(?<![\w.$]){re.escape(name)}\b(\s*\()?", code)
    if name[:1].isupper():
        return spec
    if "function" in kinds:
        return spec if uses and all(uses) else None
    if "class" not in kinds and name not in VARIABLES:
        return spec  # a namespace
    return None


def declare(names, free, code):
    lines = []
    for n in sorted(free):
        spec = importable(n, code, names)
        lines.append(f'import {{ {n} }} from "{spec}";' if spec else f"declare let {n}: any;")
    return lines


def known():
    try:
        import tomllib
    except ImportError:
        return []
    with open(KNOWN, "rb") as f:
        return tomllib.load(f).get("known", [])


def fragment(code: str, imports_first: bool = True) -> str:
    """A block that ``return``s is the body of a function the page has."""
    if not re.search(r"^return\b", code, re.M):
        return code
    lines = code.split("\n")
    head = [line for line in lines if line.startswith(("import ", "from "))]
    rest = [line for line in lines if not line.startswith(("import ", "from "))]
    return "\n".join(head + ["def fragment():"] + ["    " + line for line in rest])


def python_blocks():
    book = shared_book.Book("python")
    pages = [(p, book.render(open(p, encoding="utf-8").read(), p))
             for p in sorted(glob.glob(os.path.join(HERE, "src", "**", "*.md"), recursive=True))]
    for p in sorted(glob.glob(os.path.join(PYTHON, "docs", "src", "**", "*.md"), recursive=True)):
        text = open(p, encoding="utf-8").read()
        if os.sep + "api" + os.sep not in p and "{{#include" not in text:
            pages.append((p, text))
    fence = re.compile(r"^(\s*)```(python|py)\s*$")
    for path, text in pages:
        lines = text.split("\n")
        i = 0
        while i < len(lines):
            m = fence.match(lines[i])
            if m:
                indent, start = len(m.group(1)), i + 1
                end = start
                while not lines[end].strip().startswith("```"):
                    end += 1
                code = "\n".join(line[indent:] for line in lines[start:end])
                yield os.path.relpath(path, os.path.dirname(HERE)), start + 1, code
                i = end
            i += 1


def check_python() -> int:
    root = tempfile.mkdtemp(prefix="clausters-py-blocks-")
    try:
        index = {}
        for n, (chapter, line, code) in enumerate(python_blocks()):
            name = f"b{n:04d}.py"
            code = re.sub(r"^(\s*)\.\.\.(\s*#.*)?$", r"\1pass", code, flags=re.M)
            open(os.path.join(root, name), "w").write(fragment(code) + "\n")
            index[name] = (chapter, line)
        rules = json.load(open(os.path.join(PYTHON, "pyrightconfig.json"), encoding="utf-8"))
        config = {k: v for k, v in rules.items() if k not in ("include", "extraPaths")}
        config.update({"include": ["."], "extraPaths": [PYTHON], "reportUndefinedVariable": "none"})
        json.dump(config, open(os.path.join(root, "pyrightconfig.json"), "w"))
        out = subprocess.run(["npx", "--yes", PYRIGHT, "-p", ".", "--outputjson"],
                             capture_output=True, text=True, cwd=root)
        try:
            report = json.loads(out.stdout)
        except ValueError:
            print(f"book_blocks: pyright did not run: {out.stderr.strip()[:400]}", file=sys.stderr)
            return 2
        expected = [k for k in known() if k.get("language") == "python"]
        seen, failures = set(), []
        for d in report["generalDiagnostics"]:
            name = os.path.basename(d["file"])
            chapter, start = index[name]
            text = open(os.path.join(root, name)).read().split("\n")[d["range"]["start"]["line"]].strip()
            hit = next((k for k in expected if k["chapter"] == chapter and k["code"] in text), None)
            if hit:
                seen.add(id(hit))
                continue
            failures.append(f"{chapter}: the block at line {start}: {d['message']}\n    {text}")
        failures += [f"book_blocks.toml: `{k['code']}` in {k['chapter']} no longer fails: take its entry away"
                     for k in expected if id(k) not in seen]
        if failures:
            print("\n".join(failures), file=sys.stderr)
            print(f"book_blocks: {len(failures)} problem(s) in the Python book's code", file=sys.stderr)
            return 1
        print(f"book_blocks: {len(index)} Python blocks check")
        return 0
    finally:
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    which = sys.argv[1:] or ["python", "web"]
    status = 0
    if "python" in which:
        status = max(status, check_python())
    if "web" in which:
        status = max(status, check_web())
    return status


def check_web() -> int:
    if not os.path.exists(TSC):
        print("book_blocks: clients/web/node_modules has no tsc (npm ci there) -- nothing was checked",
              file=sys.stderr)
        return 2
    root = tempfile.mkdtemp(prefix="clausters-ts-blocks-")
    try:
        names = exports(root)
        index = {}
        for n, (chapter, line, code, made) in enumerate(blocks()):
            name = f"b{n:04d}.ts"
            body = module_of(code, made)
            head = []
            for cls in re.findall(r"^//needs (\w+)$", body, re.M):
                spec = importable(cls, "", names)
                # The block may import the class itself.
                imported = re.search(rf"^import\b[^;]*\b{cls}\b", body, re.M)
                body = re.sub(rf"^//needs {cls}$",
                              f'import {{ {cls} }} from "{spec}";'
                              if spec and not imported else "", body, count=1, flags=re.M)
            for g in SHADOWED:
                if re.search(rf"(?<![\w.$]){g}\b", body) and not declares(body, g):
                    head += declare(names, {g}, body)
            open(os.path.join(root, name), "w").write("\n".join(head + [body]))
            index[name] = (chapter, line, len(head) + body.count("\n", 0, body.find(code[:20])) if code else 0)
        tsconfig(root, "b*.ts")
        for _ in range(4):
            free = {}
            for line in tsc(root):
                m = ERROR.match(line)
                if m and m.group(4) in UNDECLARED:
                    free.setdefault(m.group(1), set()).add(re.search(r"'([^']+)'", m.group(5)).group(1))
            if not free:
                break
            for name, missing in free.items():
                path = os.path.join(root, name)
                body = open(path).read()
                open(path, "w").write("\n".join(declare(names, missing, body) + [body]))
        expected = [k for k in known() if k.get("language", "web") == "web"]
        seen, failures = set(), []
        for line in tsc(root):
            m = ERROR.match(line)
            if not m:
                if line and not line.startswith(" ") and "error TS" in line:
                    failures.append(line)
                continue
            name, row, _, code, message = m.groups()
            chapter, start, _ = index[name]
            text = open(os.path.join(root, name)).read().split("\n")[int(row) - 1].strip()
            hit = next((k for k in expected if k["chapter"] == chapter and k["code"] in text
                        and k["error"] == code), None)
            if hit:
                seen.add(id(hit))
                continue
            failures.append(f"{chapter}: the block at line {start}: {code} {message}\n    {text}")
        stale = [k for k in expected if id(k) not in seen]
        for k in stale:
            failures.append(f"book_blocks.toml: `{k['code']}` in {k['chapter']} no longer fails: "
                            f"take its entry away")
        if failures:
            print("\n".join(failures), file=sys.stderr)
            print(f"book_blocks: {len(failures)} problem(s) in the web book's TypeScript", file=sys.stderr)
            return 1
        print(f"book_blocks: {len(index)} TypeScript blocks compile")
        return 0
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
