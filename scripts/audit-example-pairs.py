#!/usr/bin/env python3
"""Check that a Python example and its page twin make the same calls.

The non-divergence rule says a pair of examples is *one example in two
languages* -- same material, same names, the same calls to the same API in the
same order -- and until this existed the only thing enforcing it was somebody
reading both files. This reads both files instead.

What it compares is the pair's **ordered sequence of calls on the client
surface**: every call either file makes whose name is one the Python client or
the web client declares, in source order, with a named callback inlined where
it is handed over rather than where it is written (a ``def on_pick`` above the
call and an inline arrow inside it are the same program). Everything else --
``mkdir``, ``getElementById``, ``toFixed``, a local helper -- never enters the
sequence.

Two spellings of one call are reconciled two ways, in this order:

* **automatically**, when the difference is only the language's case
  convention: ``on_event`` is ``onEvent``, ``sample_rate`` is ``sampleRate``.
* **by declaration**, when it is not: ``docs/example-parity.md`` carries the
  idiom table, the same shape ``docs/bindings.md`` keeps for the ABI. A row
  either pairs two names (``live`` / ``embed``) or drops one that has no
  counterpart, and says which of ``idiom`` / ``n/a`` / ``gap`` it is.

An undeclared difference is a failure and prints as a diff of the two
sequences. A row marked ``gap`` is a difference somebody decided to keep, so it
is reported and does not fail; ``--strict`` fails on those too.

    scripts/audit-example-pairs.py [--strict] [--verbose] [pattern...]

A pattern is matched against the pair's name (``views/rulers``), so a single
pair is audited with ``scripts/audit-example-pairs.py views/rulers``.
"""

from __future__ import annotations

import argparse
import ast
import difflib
import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY_EXAMPLES = ROOT / "clients" / "python" / "examples"
WEB_EXAMPLES = ROOT / "clients" / "web" / "examples"
PY_PACKAGE = ROOT / "clients" / "python" / "clausters"
WEB_PACKAGE = ROOT / "clients" / "web" / "src"
PARITY_DOC = ROOT / "docs" / "example-parity.md"

# Receivers whose methods belong to the language, never to a client: a call on
# one of them is dropped before the surface filter is consulted, which is what
# keeps `Math.round` from colliding with the client's own `round`. A name the
# file binds to something one of these built counts too (`_py_platform_locals`),
# so `rng = random.Random(2026)` makes `rng.uniform(...)` the standard
# library's and not the client's generator of the same name.
PLATFORM_RECEIVERS = {
    # JavaScript
    "Math", "Number", "String", "Object", "Array", "JSON", "Date", "console", "document", "window", "navigator", "performance", "globalThis",
    "location", "history", "localStorage", "customElements", "URL", "Boolean",
    "Map", "Set", "WeakMap", "Reflect", "Intl", "process", "Symbol", "BigInt",
    # Python
    "os", "sys", "time", "math", "random", "json", "tempfile", "pathlib",
    "shutil", "struct", "itertools", "functools", "threading", "argparse",
    "asyncio", "subprocess", "textwrap", "collections", "array", "wave",
    "Path", "np", "numpy", "traceback", "signal", "atexit", "datetime",
}

# Names that are the language's own when called bare, with no receiver to say
# otherwise. They are what makes `int(x)` and `Number(x)` -- or `len` and
# `.length` -- invisible to a comparison that is about the client's verbs; a
# call the examples do mean, like `builtins.round`, keeps its receiver and so
# never reaches this set.
PLATFORM_CALLEES = {
    # Python builtins
    "int", "float", "str", "bool", "list", "dict", "tuple", "set", "len",
    "print", "range", "enumerate", "open", "sorted", "sum", "min", "max",
    "abs", "round", "zip", "isinstance", "getattr", "setattr", "hasattr",
    "repr", "format", "type", "id", "input", "iter", "next", "any", "all",
    "map", "filter", "reversed", "bytes", "bytearray", "memoryview", "divmod",
    "pow", "hex", "chr", "ord", "vars", "dir", "super", "exit", "globals",
    # JavaScript globals
    "Number", "String", "Boolean", "Array", "Object", "parseInt", "parseFloat",
    "isNaN", "isFinite", "alert", "confirm", "prompt", "setTimeout",
    "setInterval", "clearTimeout", "clearInterval", "requestAnimationFrame",
    "cancelAnimationFrame", "queueMicrotask", "structuredClone", "fetch",
    "encodeURIComponent", "decodeURIComponent", "btoa", "atob", "Error",
    "TypeError", "RangeError", "Float32Array", "Float64Array", "Uint8Array",
    "Int16Array", "Int32Array", "Uint16Array", "Uint32Array", "ArrayBuffer",
    "DataView", "BigInt", "Symbol", "Proxy", "AbortController", "Blob",
    "FileReader", "Image", "Audio", "Worker", "URLSearchParams", "Promise",
    # node, for the example generators that author a bundle
    "writeFile", "readFile", "mkdir", "readdir", "stat", "unlink", "dirname",
    "basename", "resolve", "fileURLToPath",
}

# Container and promise methods: the language's, whatever they are called on.
# They are listed rather than deduced because several of them are also client
# verbs -- `map` is a range map, `push` writes a tempo change, `set` retunes a
# widget -- so a call is only dropped when its receiver is something the script
# built as a list, a string or a promise (`_js_containers`), never on the
# strength of the name alone.
CONTAINER_METHODS = {
    "map", "filter", "forEach", "reduce", "push", "pop", "shift", "unshift",
    "slice", "splice", "concat", "join", "split", "indexOf", "lastIndexOf",
    "includes", "find", "findIndex", "some", "every", "sort", "reverse",
    "flat", "flatMap", "fill", "at", "keys", "values", "entries", "has",
    "get", "set", "add", "delete", "clear", "then", "catch", "finally",
    "toFixed", "toString", "trim", "replace", "padStart", "padEnd", "repeat",
    "startsWith", "endsWith", "toLowerCase", "toUpperCase", "charAt", "subarray",
}

# The same, for the Python side: methods of the standard library's own objects
# that collide with a client verb (`pathlib.Path.resolve`, a dict's `get`).
PY_CONTAINER_METHODS = {
    "resolve", "mkdir", "exists", "unlink", "rmdir", "get", "items", "values",
    "keys", "append", "extend", "insert", "pop", "sort", "index", "count",
    "join", "split", "strip", "format", "encode", "decode", "startswith",
    "endswith", "lower", "upper", "replace", "read", "write", "readlines",
    "read_bytes", "write_bytes", "read_text", "write_text", "tobytes",
}

# The arithmetic Python writes as an operator and TypeScript, which has no
# operator overloading, writes as a method. `sine(freq) * env` and
# `sine(freq).mul(env)` are the same graph, so the method has to go -- but the
# same names are real verbs elsewhere (`Timeline.add(at, item)`,
# `Patch.add(def)`), and a name is not evidence. What separates them is arity:
# an operator method takes one operand or none, so **both** sides drop a call
# of these names with at most one argument. Symmetric on purpose: dropping on
# one side only would invent a difference, and dropping on both can at worst
# miss one.
OPERATOR_METHODS = {
    "add", "sub", "mul", "div", "mod", "pow", "neg", "abs",
    "gt", "lt", "ge", "le", "eq", "ne",
    "bitand", "bitor", "bitxor", "leftshift", "rightshift",
}

# Names a client owns *and* a standard container owns, told apart by how many
# arguments they were given -- `bus.get()` reads a control bus, `d.get(k)` and
# `d.get(k, default)` read a mapping. Dropped on both sides, like the operator
# methods above, so the rule can never invent a difference.
ARITY_CONTAINER_METHODS = {"get": (1, 2), "map": (1, 1)}

# Methods no client defines on anything: whatever they are called on, they are
# the language's, so a receiver the scanner cannot name does not save them.
# (`map` and `at` are *not* here -- a node maps a control to a bus and a moment
# has an `at` -- which is what the arity table above is for.)
LANGUAGE_METHODS = {
    "filter", "join", "slice", "concat", "includes", "split", "match", "count",
    "sort", "some", "every", "forEach", "reduce", "indexOf", "lastIndexOf",
    "flatMap", "padStart", "padEnd", "trim", "replace", "repeat", "startsWith",
    "endsWith", "toLowerCase", "toUpperCase", "charAt", "toFixed", "toString",
    "then", "catch", "finally", "splice", "subarray", "pop", "shift",
    "unshift", "strip", "encode", "decode", "lower", "upper", "startswith",
    "endswith", "readlines", "tobytes",
}

JS_KEYWORDS = {
    "if", "for", "while", "switch", "catch", "return", "typeof", "new",
    "await", "of", "in", "function", "do", "else", "delete", "void",
    "instanceof", "yield", "throw", "case", "with", "import", "export",
    "class", "extends", "super", "this", "let", "const", "var", "async",
    "try", "finally", "break", "continue", "default", "as", "from",
    "true", "false", "null", "undefined",
}


# --------------------------------------------------------------------------
# The two extractors
# --------------------------------------------------------------------------


def _py_calls(path: Path) -> list[str]:
    """The Python file's call names, in source order, callbacks inlined."""
    tree = ast.parse(path.read_text(encoding="utf-8"), str(path))
    platform, elements = _py_platform_locals(tree)
    aliases = _py_aliases(tree)

    # Every function bound to a name, with the span of its whole definition.
    funcs: dict[str, tuple[tuple[int, int], tuple[int, int]]] = {}

    class Defs(ast.NodeVisitor):
        def _add(self, node) -> None:
            start = (node.lineno, node.col_offset)
            end = (node.end_lineno or node.lineno, node.end_col_offset or 0)
            funcs.setdefault(node.name, (start, end))
            self.generic_visit(node)

        visit_FunctionDef = _add
        visit_AsyncFunctionDef = _add

        def visit_Assign(self, node: ast.Assign) -> None:
            # `rms = lambda a: ...` is a named function, the same way
            # `const rms = (a) => ...` is one on the other side.
            if isinstance(node.value, ast.Lambda):
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        start = (node.lineno, node.col_offset)
                        end = (node.end_lineno or node.lineno,
                               node.end_col_offset or 0)
                        funcs.setdefault(target.id, (start, end))
            self.generic_visit(node)

    Defs().visit(tree)

    # What each function's parameters bind, so a parameter named after a
    # module-level function is not read as a reference to it.
    shadowed = []
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
            args = node.args
            bound = {a.arg for a in
                     args.posonlyargs + args.args + args.kwonlyargs}
            for extra in (args.vararg, args.kwarg):
                if extra is not None:
                    bound.add(extra.arg)
            if bound:
                shadowed.append(((node.lineno, node.col_offset),
                                 (node.end_lineno or node.lineno,
                                  node.end_col_offset or 0), bound))

    # Two streams of events, both keyed by position: a call, and a reference to
    # a function's name (which is where that function's own stream is spliced).
    events: list[tuple[tuple[int, int], str, str]] = []

    class Walk(ast.NodeVisitor):
        def visit_Call(self, node: ast.Call) -> None:
            name, receiver = _py_callee(node.func)
            root = _py_root(node.func)
            argc = len(node.args) + len(node.keywords)
            if (name is not None and not (receiver is None and name in funcs)
                    and receiver not in PLATFORM_RECEIVERS
                    and not (receiver is None and name in PLATFORM_CALLEES)
                    and not (name in OPERATOR_METHODS and argc <= 1)
                    and not (argc in range(*_span(ARITY_CONTAINER_METHODS.get(name))))
                    and receiver not in platform
                    and not (receiver is not None and name in LANGUAGE_METHODS)
                    and not (name in PY_CONTAINER_METHODS
                             and (receiver in elements
                                  or root in platform | PLATFORM_RECEIVERS
                                  | PLATFORM_CALLEES))):
                # Keyed on where the *callee's name* ends, which is the order
                # the page's tokens read in: `view(canvas())` is view then
                # canvas, and `Server().boot()` is Server then boot.
                events.append((_end(node.func), "call", aliases.get(name, name)))
            self.generic_visit(node)

        def visit_Name(self, node: ast.Name) -> None:
            pos = _end(node)
            if any(start < pos < stop and node.id in bound
                   for start, stop, bound in shadowed):
                return          # a parameter of the function it sits in
            if isinstance(node.ctx, ast.Load) and node.id in funcs:
                events.append((_end(node), "ref", node.id))

    Walk().visit(tree)
    events.sort(key=lambda e: (e[0], 0 if e[1] == "ref" else 1))
    return _splice(events, funcs)


def _span(bounds: "tuple[int, int] | None") -> tuple[int, int]:
    """A `range` that is empty when the name is not arity-disambiguated."""
    return (bounds[0], bounds[1] + 1) if bounds else (0, 0)


def _end(node: ast.expr) -> tuple[int, int]:
    """Where a node ends, as a sort key."""
    return (node.end_lineno or node.lineno, node.end_col_offset or 0)


def _py_callee(func: ast.expr) -> tuple[str | None, str | None]:
    """``(name, receiver)``; the receiver is ``None`` only for a bare call.

    A method on something the audit cannot name -- ``scene(path).open()`` --
    reports an empty receiver rather than none, so it is never mistaken for a
    builtin of the same name.
    """
    if isinstance(func, ast.Name):
        return func.id, None
    if isinstance(func, ast.Attribute):
        receiver = func.value.id if isinstance(func.value, ast.Name) else ""
        return func.attr, receiver
    return None, None


def _py_aliases(tree: ast.AST) -> dict[str, str]:
    """`from ... import Event as SeqEvent` -> `{"SeqEvent": "Event"}`.

    A file that renames an import to keep two `Event`s apart is still calling
    the one the client declares, and that is the name the other side spells.
    """
    out: dict[str, str] = {}
    for node in ast.walk(tree):
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            for alias in node.names:
                if alias.asname:
                    out[alias.asname] = alias.name.split(".")[-1]
    return out


def _py_root(node: ast.expr) -> str | None:
    """The name at the head of an attribute/call chain, if it has one.

    ``pathlib.Path(__file__).resolve()`` is rooted at ``pathlib``, which is what
    says the ``resolve`` is the standard library's and not a client's.
    """
    while True:
        if isinstance(node, ast.Attribute):
            node = node.value
        elif isinstance(node, ast.Call):
            node = node.func
        elif isinstance(node, ast.Subscript):
            node = node.value
        elif isinstance(node, ast.Name):
            return node.id
        else:
            return None


def _py_platform_locals(tree: ast.AST) -> tuple[set[str], set[str]]:
    """Names the script binds to something the standard library built, and the
    names a loop over one of those binds.

    The first set is trusted for **any** method (`rng.uniform(...)` is the
    standard library's generator, not the client's); the second only for the
    container methods, because a loop variable is one element of something and
    what that element *is* the scanner cannot say. Neither is scoped -- a name
    is a name here -- so the rules stay narrow on purpose: an inference that
    leaked would silently drop a client call.
    """
    names: set[str] = set()
    elements: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.With, ast.AsyncWith)):
            # `with open(path, "w") as f` binds a file object, so `f.write(...)`
            # is the standard library's.
            for item in node.items:
                target = item.optional_vars
                if (isinstance(target, ast.Name)
                        and _py_root(item.context_expr)
                        in PLATFORM_RECEIVERS | PLATFORM_CALLEES):
                    names.add(target.id)
            continue
        if isinstance(node, (ast.For, ast.AsyncFor, ast.comprehension)):
            iterable = node.iter
            if (_py_root(iterable) in PLATFORM_RECEIVERS | PLATFORM_CALLEES
                    or isinstance(iterable, (ast.List, ast.Tuple, ast.Dict,
                                             ast.ListComp, ast.SetComp))):
                elements.update(n.id for n in ast.walk(node.target)
                                if isinstance(n, ast.Name))
            continue
        if not isinstance(node, ast.Assign):
            continue
        targets = [t.id for t in node.targets if isinstance(t, ast.Name)]
        value = node.value
        if isinstance(value, (ast.List, ast.Dict, ast.Set, ast.Tuple,
                              ast.ListComp, ast.DictComp, ast.SetComp,
                              ast.JoinedStr)):
            names.update(targets)
        elif isinstance(value, ast.Constant) and isinstance(value.value, str):
            names.update(targets)
        elif _py_root(value) in PLATFORM_RECEIVERS | PLATFORM_CALLEES:
            names.update(targets)
    return names, elements


def _splice(
    events: list[tuple[tuple[int, int], str, str]],
    funcs: dict[str, tuple[object, object]],
) -> list[str]:
    """Flatten position-keyed events, inlining each function where it is used.

    A call inside a named function is emitted at the point the name is handed
    over, not where the function was written -- so a script that spells a
    handler as a ``def`` above the call reads like a page that spells it as an
    arrow inside it. A function nobody refers to is emitted at its definition,
    which is where it already was.
    """
    # A function nobody ever refers to -- a cell a REPL calls by hand, a page's
    # button handler -- is emitted where it was written; one that is referred
    # to is emitted at the first reference instead.
    referenced = {name for _pos, kind, name in events if kind == "ref"}
    events = list(events) + [(start, "def", name)
                             for name, (start, _end) in funcs.items()
                             if name not in referenced]
    events.sort(key=lambda e: (e[0], {"def": 0, "ref": 1, "call": 2}[e[1]]))

    inside: dict[str, list[tuple[tuple[int, int], str, str]]] = {n: [] for n in funcs}
    outside: list[tuple[tuple[int, int], str, str]] = []
    for ev in events:
        pos = ev[0]
        owner = None
        best: tuple[object, object] | None = None
        for name, (start, end) in funcs.items():
            if ev[1] == "def" and name == ev[2]:
                continue        # a definition marker sits outside its own body
            # `<= end`: an event keyed on where it *ends* can land exactly on
            # the end of the function holding it -- `return handler` is the
            # last thing in its `def`, and both end at the same column.
            if start < pos <= end and (best is None or start > best[0]):  # type: ignore[operator]
                owner, best = name, (start, end)
        if owner is None:
            outside.append(ev)
        else:
            inside[owner].append(ev)

    emitted: set[str] = set()

    def flatten(stream: list[tuple[tuple[int, int], str, str]]) -> list[str]:
        out: list[str] = []
        for _pos, kind, name in stream:
            if kind == "call":
                out.append(name)
            elif name not in emitted:
                emitted.add(name)
                out.extend(flatten(inside[name]))
        return out

    return flatten(outside)


# The page half. TypeScript 7 ships no JavaScript compiler API and this
# repository vendors no parser, so the page's scripts are read by a scanner
# that knows just enough syntax to place a call: strings, template literals,
# comments and regular expressions are skipped, and a call is an identifier
# followed by `(`. It is the same two streams the Python half builds.

# What a removed comment, string or regex leaves behind: a token, so nothing
# on either side of it becomes adjacent, and punctuation, so nothing reads as a
# name.
FILL = "~"

_JS_TOKEN = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*|[^\sA-Za-z0-9_$]")


def _js_strip(src: str) -> str:
    """Blank out comments, strings and regexes, keeping every offset in place.

    A template literal keeps its **holes**: `${...}` is code and is read as
    code, the literal text around it is not. That is what stops the words in a
    log line from reading as calls.

    What is removed is replaced by `FILL`, not by a space: two identifiers that
    a string used to separate must not become adjacent, or `take.frames` and
    the `(` that opens the next hole would read as a call to `frames`.
    """
    out = list(src)
    n = len(src)
    i = 0
    prev = ""
    # Each entry is a template literal whose hole we are inside; the number is
    # the brace depth at which that hole closes.
    templates: list[int] = []
    depth = 0

    def blank(a: int, b: int) -> None:
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = FILL

    while i < n:
        c = src[i]
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
            continue
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            blank(i, j)
            i = j
            continue
        if c in "\"'":
            quote = c
            j = i + 1
            while j < n and src[j] != quote:
                j += 2 if src[j] == "\\" else 1
            blank(i, min(j + 1, n))
            i = j + 1
            prev = "x"
            continue
        if c == "`":
            i = _js_template(src, out, i, blank, templates, depth)
            prev = "x"
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            if templates and templates[-1] == depth:
                # The hole closes: back into the literal text of its template.
                templates.pop()
                out[i] = FILL
                i = _js_template_body(src, out, i + 1, blank, templates, depth)
                prev = "x"
                continue
            depth -= 1
        elif c == "/" and prev in "(,=:[!&|?{};+-*%~^<>":
            j = i + 1
            in_class = False
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == "[":
                    in_class = True
                elif src[j] == "]":
                    in_class = False
                elif src[j] == "/" and not in_class:
                    break
                elif src[j] == "\n":
                    break
                j += 1
            if j < n and src[j] == "/":
                blank(i, j + 1)
                i = j + 1
                prev = "x"
                continue
        if not c.isspace():
            prev = c
        i += 1
    return "".join(out)


def _js_template(src, out, i, blank, templates, depth):
    """Enter a template literal at its opening backtick."""
    out[i] = FILL
    return _js_template_body(src, out, i + 1, blank, templates, depth)


def _js_template_body(src, out, i, blank, templates, depth):
    """Blank a template's literal text up to its end or its next hole."""
    n = len(src)
    while i < n:
        if src[i] == "\\":
            blank(i, i + 2)
            i += 2
            continue
        if src[i] == "`":
            out[i] = FILL
            return i + 1
        if src[i] == "$" and i + 1 < n and src[i + 1] == "{":
            blank(i, i + 2)
            templates.append(depth)
            return i + 2
        if out[i] != "\n":
            out[i] = FILL
        i += 1
    return n


def _js_scripts(path: Path) -> list[tuple[int, str]]:
    text = path.read_text(encoding="utf-8")
    if path.suffix != ".html":
        return [(0, text)]
    blocks: list[tuple[int, str]] = []
    for m in re.finditer(r"<script\b([^>]*)>(.*?)</script>", text, re.S | re.I):
        if re.search(r"\bsrc\s*=", m.group(1), re.I):
            continue
        blocks.append((m.start(2), m.group(2)))
    return blocks


def _js_calls(path: Path) -> list[str]:
    names: list[str] = []
    for _offset, block in _js_scripts(path):
        src = _js_strip(block)
        tokens = [(m.start(), m.group()) for m in _JS_TOKEN.finditer(src)]
        funcs = _js_functions(src, tokens)
        shadows = _js_params(src, tokens, funcs)
        containers = _js_containers(tokens)
        events: list[tuple[tuple[int, int], str, str]] = []
        for idx, (pos, tok) in enumerate(tokens):
            if not tok[:1].isalpha() and tok[:1] not in "_$":
                continue
            nxt = tokens[idx + 1][1] if idx + 1 < len(tokens) else ""
            prev = tokens[idx - 1][1] if idx else ""
            spread = prev == "." and idx >= 2 and tokens[idx - 2][1] == "."
            # The name in `function f`, `function* f` or `const f = ...` is
            # the declaration: neither a call nor a use of one.
            if (prev in ("function", "const", "let", "var")
                    or (prev == "*" and idx >= 2
                        and tokens[idx - 2][1] == "function")):
                continue
            if (tok in funcs and (prev != "." or spread)
                    and not _js_is_param(tokens, idx)
                    and not any(start <= pos < end and tok in bound
                                for start, end, bound in shadows)):
                # A locally written function is inlined where it is used --
                # called or handed over, the two are the same program.
                if nxt != ":":
                    events.append(((pos, 0), "ref", tok))
            elif nxt == "(" and tok not in JS_KEYWORDS:
                receiver = tokens[idx - 2][1] if (prev == "." and idx >= 2) else None
                if (receiver not in PLATFORM_RECEIVERS
                        and not (receiver is None and tok in PLATFORM_CALLEES)
                        and not (tok in OPERATOR_METHODS
                                 and _js_argc(tokens, idx + 1) <= 1)
                        and not (_js_argc(tokens, idx + 1)
                                 in range(*_span(ARITY_CONTAINER_METHODS.get(tok))))
                        and not (receiver is not None and tok in LANGUAGE_METHODS)
                        and not (tok in CONTAINER_METHODS
                                 and _js_on_container(tokens, idx, containers))):
                    events.append(((pos, 0), "call", tok))
        events.sort(key=lambda e: (e[0], 0 if e[1] == "ref" else 1))
        names.extend(_splice(events, funcs))
    return names


# What a call on a container is written on: a list the page built, a string, or
# the closing bracket of a literal. `]` and the string FILL stand for the
# literal forms, `null` for a chain whose head the scanner cannot name.
# Methods whose result is a list or a string whatever they were called on, so a
# name bound to a chain ending in one is a container too.
_CONTAINER_PRODUCERS = {"split", "slice", "concat", "filter", "map", "flat",
                        "flatMap", "keys", "values", "entries", "from", "join",
                        "match", "replace", "trim"}

_CONTAINER_CTORS = {"Map", "Set", "Array", "WeakMap", "Float32Array",
                    "Float64Array", "Uint8Array", "Int16Array", "Int32Array",
                    "Uint16Array", "Uint32Array", "Object", "Promise", "JSON",
                    "URLSearchParams", "Headers", "FormData"}


def _js_on_container(tokens: list[tuple[int, str]], idx: int, containers: set[str]) -> bool:
    """Is the call at ``idx`` made on something the standard library built?

    The receiver is walked back through the chain: a name the script bound to a
    list, a literal `[...]` or a string, or the result of another container
    method -- `line.split(...).filter(Boolean).map(...)` is three of them and
    none is a client verb.
    """
    if idx < 2 or tokens[idx - 1][1] != ".":
        return False
    j = idx - 2
    tok = tokens[j][1]
    if tok in ("]", FILL):
        return True
    if _is_ident(tok):
        return tok in containers
    if tok != ")":
        return False
    depth = 0
    while j >= 0:                       # back to the `(` this `)` closes
        if tokens[j][1] in ")]}":
            depth += 1
        elif tokens[j][1] in "([{":
            depth -= 1
            if depth == 0:
                break
        j -= 1
    if j <= 0 or not _is_ident(tokens[j - 1][1]):
        return False
    previous = tokens[j - 1][1]
    if previous in _CONTAINER_PRODUCERS or previous in containers:
        return True                     # a list, whatever it was made from
    return (previous in CONTAINER_METHODS
            and _js_on_container(tokens, j - 1, containers))


def _js_params(src: str, tokens: list[tuple[int, str]],
                funcs: dict[str, tuple[tuple[int, int], tuple[int, int]]]):
    """Each function's span and the names its parameters bind.

    A name a parameter binds means the parameter inside that function, not the
    file's function of the same name -- `const log = (line) => ... line ...`
    would otherwise splice the `line` generator into every log call.
    """
    out = []
    for name, (start, end) in funcs.items():
        opening = src.find("(", start[0])
        if opening < 0 or opening > end[0]:
            continue
        depth = 0
        close = opening
        for i in range(opening, min(end[0], len(src))):
            if src[i] == "(":
                depth += 1
            elif src[i] == ")":
                depth -= 1
                if depth == 0:
                    close = i
                    break
        bound = {m for m in re.findall(r"[A-Za-z_$][\w$]*", src[opening:close])
                 if m not in JS_KEYWORDS}
        if bound:
            out.append((start[0], end[0], bound))
    return out


def _js_is_param(tokens: list[tuple[int, str]], idx: int) -> bool:
    """Is the identifier at ``idx`` a parameter name rather than a reference?

    `const log = (line) => ...` binds `line` here; it does not mean whatever
    `line` is elsewhere in the file. Without this a page that names a parameter
    after one of its own functions splices that function in at the parameter.
    """
    prev = tokens[idx - 1][1] if idx else ""
    if prev not in ("(", ","):
        return False
    depth = 0                           # forward to the `)` that closes the list
    for j in range(idx, len(tokens)):
        tok = tokens[j][1]
        if tok in "([{":
            depth += 1
        elif tok in ")]}":
            depth -= 1
            if depth < 0:
                nxt = tokens[j + 1][1] if j + 1 < len(tokens) else ""
                if nxt == "=":          # `=>` reads as two tokens
                    return (j + 2 < len(tokens) and tokens[j + 2][1] == ">")
                # `function f(line)` / `function* f(line)`
                back = idx - 1
                while back > 0 and tokens[back][1] != "(":
                    back -= 1
                return (back >= 2 and tokens[back - 2][1] in ("function", "*")
                        or back >= 1 and tokens[back - 1][1] == "function")
    return False


def _js_argc(tokens: list[tuple[int, str]], paren: int) -> int:
    """How many arguments the call whose `(` is at ``paren`` was given."""
    depth = 0
    args = 0
    for _pos, tok in tokens[paren:]:
        if tok in "([{":
            depth += 1
            if depth == 1:
                args = 1
        elif tok in ")]}":
            depth -= 1
            if depth == 0:
                return args
        elif tok == "," and depth == 1:
            args += 1
    return args


def _js_containers(tokens: list[tuple[int, str]]) -> set[str]:
    """Names the script binds to a list, a string or a standard container.

    Every declarator of a statement, not only its first: `const pitches = [],
    durs = [], amps = []` binds three lists, and a `push` on any of them is the
    language's, not a tempo map's.
    """
    names: set[str] = {"]", FILL} | _CONTAINER_CTORS
    for idx, (_pos, tok) in enumerate(tokens):
        # A declaration, or a plain assignment to a name declared elsewhere --
        # `let samples` at the top and `samples = Array.from(...)` inside a
        # function is one list, and the audit has no scopes to tell it apart.
        if tok not in ("const", "let", "var"):
            if not (_is_ident(tok) and idx + 1 < len(tokens)
                    and tokens[idx + 1][1] == "="
                    and (idx == 0 or tokens[idx - 1][1] not in (".", "=", "!", "<", ">"))
                    and (idx + 2 >= len(tokens) or tokens[idx + 2][1] != "=")):
                continue
            j = idx
        else:
            j = idx + 1
        depth = 0
        while j + 1 < len(tokens) and tokens[j][1] != ";":
            here = tokens[j][1]
            if here in "([{":
                depth += 1
            elif here in ")]}":
                depth -= 1
                if depth < 0:
                    break
            elif depth == 0 and _is_ident(here) and tokens[j + 1][1] == "=":
                first = tokens[j + 2][1] if j + 2 < len(tokens) else ""
                second = tokens[j + 3][1] if j + 3 < len(tokens) else ""
                # No aliasing rule (`const b = a`): one *element* of a
                # container is not a container, and `const synth =
                # voices.get(p)` would otherwise make `synth.set(...)` a Map's.
                if (first in ("[", FILL) or first in _CONTAINER_CTORS
                        or (first == "new" and second in _CONTAINER_CTORS)):
                    names.add(here)
                else:
                    # A chain that ends in a container method builds a
                    # container: `line.split(...).filter(Boolean)` is a list,
                    # whatever `line` was.
                    k = j + 2
                    while k < len(tokens) and tokens[k][1] not in (";", ","):
                        if (tokens[k][1] in _CONTAINER_PRODUCERS
                                and tokens[k - 1][1] == "."):
                            names.add(here)
                            break
                        k += 1
            j += 1
    return names


def _js_functions(src: str, tokens: list[tuple[int, str]]) -> dict[str, tuple[tuple[int, int], tuple[int, int]]]:
    """Named function bodies in a script, as spans, for the inlining rule."""
    spans: dict[str, tuple[tuple[int, int], tuple[int, int]]] = {}

    def match_brace(start: int) -> int:
        depth = 0
        i = start
        while i < len(src):
            if src[i] == "{":
                depth += 1
            elif src[i] == "}":
                depth -= 1
                if depth == 0:
                    return i
            i += 1
        return len(src)

    for idx, (pos, tok) in enumerate(tokens):
        name = None
        body_start = None
        if tok == "function" and idx + 1 < len(tokens):
            # `function f` and the generator's `function* f` alike.
            at = idx + 2 if tokens[idx + 1][1] == "*" else idx + 1
            if at < len(tokens) and _is_ident(tokens[at][1]):
                name = tokens[at][1]
                brace = src.find("{", tokens[at][0])
                if brace >= 0:
                    body_start = brace
        elif tok in ("const", "let", "var") and idx + 2 < len(tokens) and _is_ident(tokens[idx + 1][1]) and tokens[idx + 2][1] == "=":
            arrow = src.find("=>", tokens[idx + 2][0])
            fn = src.find("function", tokens[idx + 2][0])
            semi = _statement_end(src, tokens[idx + 2][0])
            if 0 <= arrow < semi and _js_top_level(src, tokens[idx + 2][0], arrow):
                name = tokens[idx + 1][1]
                rest = arrow + 2
                while rest < len(src) and src[rest].isspace():
                    rest += 1
                # A block body is braced; an expression body runs to the end of
                # the statement, and both are the function.
                if rest < len(src) and src[rest] == "{":
                    body_start = rest
                else:
                    spans.setdefault(name, ((pos, 0), (semi, 0)))
                    continue
            elif 0 <= fn < semi:
                name = tokens[idx + 1][1]
                brace = src.find("{", fn)
                body_start = brace if brace >= 0 else None
        if name and body_start is not None:
            spans.setdefault(name, ((pos, 0), (match_brace(body_start) + 1, 0)))
    return spans


def _js_top_level(src: str, start: int, target: int) -> bool:
    """Is ``target`` reached from ``start`` without entering a bracket?

    `const f = (x) => ...` is a function; `const rows = Array.from(n, (_, i) =>
    ...)` is a list, and its arrow belongs to the call, not to the name.
    """
    depth = 0
    for i in range(start, target):
        c = src[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
    return depth == 0


def _is_ident(tok: str) -> bool:
    return bool(tok) and (tok[0].isalpha() or tok[0] in "_$") and tok not in JS_KEYWORDS


def _statement_end(src: str, start: int) -> int:
    """Where a `const f = ...` statement ends: the `;` at its own nesting."""
    depth = 0
    for i in range(start, len(src)):
        c = src[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth < 0:
                return i
        elif c == ";" and depth == 0:
            return i
    return len(src)


# --------------------------------------------------------------------------
# The two surfaces
# --------------------------------------------------------------------------


def python_surface() -> set[str]:
    names: set[str] = set()
    for path in sorted(PY_PACKAGE.rglob("*.py")):
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"), str(path))
        except SyntaxError:
            continue
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                if not node.name.startswith("_"):
                    names.add(node.name)
    return names


_TS_EXPORT = re.compile(
    r"^\s*export\s+(?:async\s+)?(?:declare\s+)?"
    r"(?:function|class|const|let|var|type|interface|enum)\s+([A-Za-z_$][\w$]*)",
    re.M,
)
_TS_EXPORT_LIST = re.compile(r"^\s*export\s*\{([^}]*)\}", re.M | re.S)
_TS_MEMBER = re.compile(
    r"^[ \t]+(?:public\s+|readonly\s+|static\s+|async\s+|abstract\s+|override\s+)*"
    r"([A-Za-z_$][\w$]*)\s*[(<:?]",
    re.M,
)


def web_surface() -> set[str]:
    names: set[str] = set()
    for path in sorted(WEB_PACKAGE.rglob("*.ts")):
        text = path.read_text(encoding="utf-8")
        names.update(_TS_EXPORT.findall(text))
        for group in _TS_EXPORT_LIST.findall(text):
            for item in group.split(","):
                item = item.strip().split(" as ")[0].strip()
                if item and re.fullmatch(r"[A-Za-z_$][\w$]*", item):
                    names.add(item)
        for line in text.splitlines():
            if re.match(r"^\s*(private|#|//|\*)", line):
                continue
            m = _TS_MEMBER.match(line)
            if m:
                names.add(m.group(1))
    return names


def camel(name: str) -> str:
    """`sample_rate` -> `sampleRate`; a name with no underscore is unchanged.

    A trailing underscore is not a word break: `in_` avoids a keyword and both
    clients spell it that way, so it stays.
    """
    if name.startswith("_") or "_" not in name.strip("_"):
        return name
    head, *tail = name.split("_")
    return head + "".join(part[:1].upper() + part[1:] if part else "_"
                          for part in tail)


# --------------------------------------------------------------------------
# The declared tables
# --------------------------------------------------------------------------


@dataclass
class Idiom:
    python: str | None
    web: str | None
    verdict: str
    note: str


def _table_rows(section: str) -> list[list[str]]:
    """Every body row of every Markdown table in ``section``.

    A header is recognized by the `|---|` rule under it, not by what it says --
    a row whose first cell is the word "python" is a row, and one of the tables
    here is full of them.
    """
    lines = [line.strip() for line in section.splitlines()]
    rows = []
    for i, line in enumerate(lines):
        if not line.startswith("|") or set(line) <= set("|- :"):
            continue
        following = lines[i + 1] if i + 1 < len(lines) else ""
        if following.startswith("|") and set(following) <= set("|- :"):
            continue                    # the header above a separator
        rows.append([c.strip() for c in line.strip("|").split("|")])
    return rows


def _cell_name(cell: str) -> str | None:
    cell = cell.strip()
    if cell in ("", "—", "-", "--"):
        return None
    m = re.search(r"`([^`]+)`", cell)
    text = m.group(1) if m else cell
    text = text.split("(")[0].strip()
    return text.split(".")[-1] or None


@dataclass
class Tables:
    """Everything `docs/example-parity.md` declares."""

    pairs: dict[str, tuple[str, str]]
    idioms: list[Idiom]
    unpaired: dict[str, str]
    allowances: dict[str, list[tuple[str, str, int]]]


def load_tables() -> Tables:
    """Read `docs/example-parity.md`: the pair table, the idiom table, the
    examples with no twin, and each pair's declared allowances."""
    text = PARITY_DOC.read_text(encoding="utf-8")
    sections = re.split(r"^## ", text, flags=re.M)
    pairs: dict[str, tuple[str, str]] = {}
    idioms: list[Idiom] = []
    unpaired: dict[str, str] = {}
    allowances: dict[str, list[tuple[str, str, int]]] = {}
    for section in sections:
        title = section.splitlines()[0].strip().lower() if section.strip() else ""
        if title.startswith("the pairs that are not spelled alike"):
            for cells in _table_rows(section):
                if len(cells) >= 2:
                    py = _bare(cells[0])
                    web = _bare(cells[1])
                    if py and web:
                        pairs[py] = (py, web)
        elif title.startswith("the idiom table"):
            for cells in _table_rows(section):
                if len(cells) >= 3:
                    idioms.append(
                        Idiom(_cell_name(cells[0]), _cell_name(cells[1]),
                              _verdict(cells[2]), cells[2])
                    )
        elif title.startswith("examples with no twin"):
            for cells in _table_rows(section):
                if len(cells) >= 2:
                    name = _bare(cells[0])
                    if name:
                        unpaired[name] = cells[1]
        elif title.startswith("what one side of a pair says alone"):
            for block in re.split(r"^### ", section, flags=re.M)[1:]:
                key = _bare(block.splitlines()[0])
                if not key:
                    continue
                rows = []
                for cells in _table_rows(block):
                    if len(cells) >= 3:
                        side = cells[0].strip().lower()
                        name = _cell_name(cells[1])
                        count = re.search(r"[x\u00d7]\s*(\d+)", cells[1])
                        n = int(count.group(1)) if count else 1
                        if "first" in cells[1].lower():
                            n = -n      # from the start, not from the end
                        if name and side in ("python", "web"):
                            rows.append((side, name, n))
                allowances.setdefault(key, []).extend(rows)
    return Tables(pairs, idioms, unpaired, allowances)


def _bare(cell: str) -> str | None:
    m = re.search(r"`([^`]+)`", cell)
    if not m:
        return None
    return m.group(1).strip()


def _verdict(note: str) -> str:
    low = note.lower()
    for verdict in ("gap", "n/a", "idiom"):
        if verdict in low:
            return verdict
    return "idiom"


# --------------------------------------------------------------------------
# Pairing and comparison
# --------------------------------------------------------------------------


def discover_pairs(declared: dict[str, tuple[str, str]], unpaired: dict[str, str]):
    """Every Python example and the page that is the same example, in order."""
    pairs: list[tuple[str, Path, Path]] = []
    lonely: list[tuple[str, Path]] = []
    for path in sorted(PY_EXAMPLES.rglob("*.py")):
        rel = path.relative_to(PY_EXAMPLES).with_suffix("")
        key = str(rel)
        if key in declared:
            web = WEB_EXAMPLES / declared[key][1]
        else:
            web = WEB_EXAMPLES / (str(rel).replace("_", "-") + ".html")
        if web.exists():
            pairs.append((key, path, web))
        elif key in unpaired:
            continue
        else:
            lonely.append((key, path))
    return pairs, lonely


def apply_idioms(names: list[str], idioms: list[Idiom], side: str) -> list[str]:
    """Normalize one side's sequence into the shared vocabulary."""
    out: list[str] = []
    for name in names:
        key = camel(name) if side == "python" else name
        mapped = key
        dropped = False
        for row in idioms:
            mine = camel(row.python) if row.python else None
            theirs = row.web
            if side == "python":
                if mine == key:
                    if theirs is None:
                        dropped = True
                    else:
                        mapped = theirs
                    break
            else:
                if theirs == key:
                    if mine is None:
                        dropped = True
                    else:
                        mapped = theirs
                    break
        if not dropped:
            out.append(mapped)
    return out


def _drop(names: list[str], name: str, count: int) -> list[str]:
    """Remove ``count`` calls of ``name``: a declared allowance.

    From the **end**, because what a pair declares is nearly always the file's
    ending -- the same verb earlier in it is ordinary work and stays. A
    negative count counts from the start, which is what a row saying `first`
    asks for.
    """
    out = list(names)
    for _ in range(abs(count)):
        order = range(len(out) - 1, -1, -1) if count > 0 else range(len(out))
        for i in order:
            if out[i] == name:
                del out[i]
                break
    return out


def filter_surface(names: list[str], surface: set[str]) -> list[str]:
    return [n for n in names if n in surface]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("patterns", nargs="*", help="audit only pairs whose name matches")
    ap.add_argument("--strict", action="store_true",
                    help="fail on a declared `gap` too, not only on an undeclared difference")
    ap.add_argument("--verbose", action="store_true",
                    help="print each pair's normalized sequence")
    args = ap.parse_args()

    tables = load_tables()
    declared, idioms, unpaired = tables.pairs, tables.idioms, tables.unpaired
    pairs, lonely = discover_pairs(declared, unpaired)
    if args.patterns:
        pairs = [p for p in pairs if any(pat in p[0] for pat in args.patterns)]

    surface = {camel(n) for n in python_surface()} | web_surface()
    for row in idioms:
        if row.web:
            surface.add(row.web)
        if row.python:
            surface.add(camel(row.python))

    failures = 0
    gaps = 0
    for key, py_path, web_path in pairs:
        py_seq = filter_surface(apply_idioms(_py_calls(py_path), idioms, "python"), surface)
        web_seq = filter_surface(apply_idioms(_js_calls(web_path), idioms, "web"), surface)
        for side, name, count in tables.allowances.get(key, []):
            seq = py_seq if side == "python" else web_seq
            seq[:] = _drop(seq, camel(name) if side == "python" else name, count)
        if args.verbose:
            print(f"# {key}\n  py : {' '.join(py_seq)}\n  web: {' '.join(web_seq)}")
        if py_seq == web_seq:
            continue
        failures += 1
        print(f"\n{key}")
        print(f"  {py_path.relative_to(ROOT)}")
        print(f"  {web_path.relative_to(ROOT)}")
        for line in difflib.unified_diff(py_seq, web_seq, "python", "web", lineterm="", n=2):
            print(f"    {line}")

    audit_all = not args.patterns
    for key, path in (lonely if audit_all else []):
        print(f"\n{key}: no page twin, and no row in docs/example-parity.md")
        print(f"  {path.relative_to(ROOT)}")
        failures += 1

    web_seen = {str(w.relative_to(WEB_EXAMPLES)) for _k, _p, w in pairs}
    for path in (sorted(WEB_EXAMPLES.rglob("*.html")) if audit_all else []):
        rel = str(path.relative_to(WEB_EXAMPLES))
        if rel.startswith("out/") or rel in web_seen or rel in unpaired:
            continue
        print(f"\n{rel}: no script twin, and no row in docs/example-parity.md")
        failures += 1

    print(f"\n{len(pairs)} pairs audited, {failures} differing, {gaps} declared gaps")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
