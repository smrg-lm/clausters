#!/usr/bin/env python3
"""Generate names-vectors.json: what each module of the package exports.

The two clients are one client in two languages, so a module of one exports
what the same module of the other does, name for name: `clausters.defs` and
`clausters/defs`, `clausters.gui.guidef` and `clausters/gui/guidef`. Nothing
checks that by itself -- a name added to one side compiles, tests and lints
clean -- so this writes the Python modules' `__all__`, spelled for TypeScript,
and `tests/names.test.ts` holds each web subpath to it.

The differences that are decided are read from `clients/docs/names.toml`,
where each says why: a module or a name only one platform has, a Python
class that TypeScript writes as a type, and the function TypeScript has for
what Python spells as a method or an operator. Every one of them is checked
here against the Python module, so an entry that no longer names anything
fails rather than lingering.

The JSON is committed; regenerate with:

    python3 gen-names-vectors.py

(from clients/web/tests/, with the Python client importable -- the repo's
.venv has it installed.)
"""

import importlib
import json
import pathlib
import sys
import tomllib

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / "clients" / "python"))
sys.path.insert(0, str(REPO / "clients" / "docs"))

import shared_book  # noqa: E402


def module_of(subpath: str) -> str:
    """The Python module a web subpath is: `./gui/guidef` is
    `clausters.gui.guidef`, `.` is `clausters`."""
    return "clausters" + ("" if subpath == "." else "." + subpath[2:].replace("/", "."))


def main() -> None:
    names = tomllib.loads((REPO / "clients" / "docs" / "names.toml").read_text())
    spelled = names.get("web", {})
    platform = names.get("platform", {})
    web_modules = set(names["modules"]["web"])
    python_modules = set(names["modules"]["python"])
    types = names.get("types", {})
    idiom = names.get("idiom", {}).get("web", {})
    subpaths = json.loads((HERE.parent / "package.json").read_text())["exports"]

    # Every public module of the Python package is a subpath of the web one,
    # unless it is a native process's.
    package = REPO / "clients" / "python" / "clausters"
    top = {p.stem if p.suffix == ".py" else p.name for p in package.iterdir()
           if not p.name.startswith("_") and (p.suffix == ".py" or (p / "__init__.py").exists())}
    covered = {module_of(s) for s in subpaths}
    missing = sorted(f"clausters.{m}" for m in top
                     if f"clausters.{m}" not in covered | python_modules)
    if missing:
        raise SystemExit(f"Python modules with no web subpath: {missing}")

    vectors = {}
    for subpath in subpaths:
        module = module_of(subpath)
        if module in web_modules:
            continue
        python = importlib.import_module(module)
        declared = getattr(python, "__all__", None)
        if declared is None:
            raise SystemExit(f"{module} declares no __all__: its surface is unsaid")
        drop = set(types.get(module, [])) | set(platform.get("python", {}).get(module, []))
        stale = sorted(drop - set(declared))
        if stale:
            raise SystemExit(f"names.toml names what {module} does not export: {stale}")
        exported = {spelled.get(n) or shared_book.camel(n) for n in declared if n not in drop}
        added = set(platform.get("web", {}).get(module, [])) | set(idiom.get(module, []))
        clash = sorted(exported & added)
        if clash:
            raise SystemExit(f"names.toml adds to {module} what it already exports: {clash}")
        vectors[subpath] = sorted(exported | added)

    out = HERE / "names-vectors.json"
    out.write_text(json.dumps(vectors, indent=2) + "\n")
    print(f"wrote {out} ({len(vectors)} modules)")


if __name__ == "__main__":
    main()
