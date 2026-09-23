"""Driver between the editor and a persistent Python session.

Protocol: one JSON request per line on stdin, one JSON message per line on the
protocol descriptor (the original stdout). fd 1 is redirected to stderr so that
no outside output (C, subprocesses) corrupts the protocol.

Requests:  {"id": 1, "op": "eval", "code": "...", "file": "a.py", "line": 10}
           {"id": 2, "op": "help", "expr": "print"}
           {"id": 3, "op": "inspect", "expr": "obj.method"}
Messages:  {"type": "out" | "err" | "result" | "help" | "done", "id": 1, "text": "..."}
           {"type": "inspect", "id": 3, "data": {...} | null}

Evaluations run on the main thread, one at a time. "help" and "inspect" are
answered by the reader thread right away, so the documentation responds even
while code is running.
"""

import ast
import builtins
import inspect
import io
import json
import linecache
import os
import pydoc
import queue
import re
import signal
import sys
import textwrap
import threading
import traceback

_proto = os.fdopen(os.dup(1), "w", buffering=1, encoding="utf-8")
os.dup2(2, 1)
_requests = sys.stdin
# Interrupting = SIGINT, even if the parent process ignores it.
signal.signal(signal.SIGINT, signal.default_int_handler)

namespace = {"__name__": "__main__", "__builtins__": builtins}


_emit_lock = threading.Lock()


def emit(kind, id=None, text="", data=None):
    msg = {"type": kind, "id": id, "text": text}
    if data is not None:
        msg["data"] = data
    line = json.dumps(msg) + "\n"
    with _emit_lock:  # both the main thread and the reader write
        _proto.write(line)
        _proto.flush()


class Stream(io.TextIOBase):
    def __init__(self, kind):
        self.kind = kind
        self.id = None

    def writable(self):
        return True

    def write(self, s):
        if s:
            emit(self.kind, self.id, s)
        return len(s)


def _no_input(prompt=""):
    raise RuntimeError("input() is not available in the editor session")


out, err = Stream("out"), Stream("err")
sys.stdout, sys.stderr, sys.stdin = out, err, io.StringIO()
builtins.input = _no_input


def run(req):
    filename = req.get("file") or "<editor>"
    line = max(int(req.get("line") or 1), 1)
    # Lets single lines of an indented block be evaluated.
    code = textwrap.dedent(req["code"])
    # Padding so that line numbers match the file's.
    source = "\n" * (line - 1) + code
    lines = source.splitlines(keepends=True)
    linecache.cache[filename] = (len(source), None, lines, filename)

    tree = ast.parse(source, filename, "exec")
    last = None
    if tree.body and isinstance(tree.body[-1], ast.Expr):
        last = tree.body.pop()
    exec(compile(tree, filename, "exec"), namespace)
    if last is not None:
        value = eval(compile(ast.Expression(last.value), filename, "eval"), namespace)
        if value is not None:
            namespace["_"] = value
            emit("result", req["id"], repr(value))


def show_error(req):
    etype, value, tb = sys.exc_info()
    # Drops the driver's own frames from the traceback.
    while tb is not None and tb.tb_frame.f_code.co_filename == __file__:
        tb = tb.tb_next
    if isinstance(value, SyntaxError):
        tb = None
    emit("err", req["id"], "".join(traceback.format_exception(etype, value, tb)))


def help_text(expr):
    try:
        obj = eval(expr, namespace)
    except Exception:
        obj = pydoc.locate(expr)
        if obj is None:
            return f"No documentation found for '{expr}'."
    return pydoc.render_doc(obj, title="%s", renderer=pydoc.plaintext)


_DOTTED = re.compile(r"[A-Za-z_]\w*(\.[A-Za-z_]\w*)*")
_MAX_DOC = 4000
# For values of these types the value is shown instead of the type's docstring.
_PLAIN = (int, float, complex, bool, str, bytes, list, tuple, dict, set, frozenset, type(None))


def resolve(expr):
    """The object a dotted name refers to, without running arbitrary code: only
    attribute lookups, and properties are not evaluated."""
    if not _DOTTED.fullmatch(expr):
        raise LookupError(expr)
    first, *rest = expr.split(".")
    if first in namespace:
        obj = namespace[first]
    elif hasattr(builtins, first):
        obj = getattr(builtins, first)
    else:
        raise LookupError(expr)
    for name in rest:
        static = inspect.getattr_static(obj, name)
        obj = static if isinstance(static, property) else getattr(obj, name)
    return obj


def own_doc(obj):
    """The docstring to show. For a class only its own (or its __init__'s): one
    inherited from a base class describes something else. For a method it is
    inherited, because an override without a docstring does what the original does."""
    if inspect.isclass(obj):
        doc = obj.__dict__.get("__doc__")
        if not (isinstance(doc, str) and doc.strip()):
            init = obj.__dict__.get("__init__")
            doc = getattr(init, "__doc__", None) if init else None
        return inspect.cleandoc(doc) if isinstance(doc, str) and doc.strip() else ""
    return inspect.getdoc(obj) or ""


def describe(expr):
    """Signature and docstring for the editor's tooltips, or None if unknown."""
    try:
        obj = resolve(expr)
    except Exception:
        return None
    name = expr.rsplit(".", 1)[-1]
    params = None
    signature = ""
    if callable(obj) and not isinstance(obj, property):
        try:
            sig = inspect.signature(obj)
            params = [str(p) for p in sig.parameters.values()]
            signature = f"{name}{sig}"
        except (TypeError, ValueError):
            signature = f"{name}(...)"
    if inspect.isclass(obj):
        kind = "class"
    elif inspect.ismodule(obj):
        kind = "module"
    elif isinstance(obj, property):
        kind = "property"
    elif callable(obj):
        kind = "function"
    else:
        kind = type(obj).__name__
    if isinstance(obj, _PLAIN):
        value = repr(obj)
        doc = value if len(value) <= 300 else value[:300] + "..."
    else:
        doc = own_doc(obj)
        if len(doc) > _MAX_DOC:
            doc = doc[:_MAX_DOC] + "\n..."
    qualname = getattr(obj, "__qualname__", None)
    if isinstance(obj, property):
        qualname = getattr(obj.fget, "__qualname__", None)
    return {
        "name": expr,
        "qualname": qualname if isinstance(qualname, str) else None,
        "kind": kind,
        "signature": signature,
        "params": params,
        "doc": doc,
    }


_jobs = queue.Queue()


def reader():
    """Reads the requests: evaluations go to the main thread's queue; documentation
    is answered right here."""
    for raw in _requests:
        try:
            req = json.loads(raw)
        except json.JSONDecodeError as e:
            emit("err", None, f"Invalid request: {e}\n")
            continue
        op = req.get("op")
        if op == "inspect":
            emit("inspect", req.get("id"), data=describe(req.get("expr", "")))
        elif op == "help":
            try:
                text = help_text(req.get("expr", ""))
            except Exception as e:
                text = f"Error while getting help: {e}"
            emit("help", req.get("id"), text)
        else:
            _jobs.put(req)
    _jobs.put(None)  # stdin closed: ends the main thread


def main():
    emit("ready", None, sys.version)
    threading.Thread(target=reader, daemon=True).start()
    while True:
        try:
            req = _jobs.get()
        except KeyboardInterrupt:
            continue  # interrupted while idle
        if req is None:
            break
        out.id = err.id = req.get("id")
        try:
            run(req)
        except KeyboardInterrupt:
            emit("err", req.get("id"), "KeyboardInterrupt\n")
        except BaseException:
            show_error(req)
        finally:
            try:
                sys.stdout.flush()
            except Exception:
                pass
            emit("done", req.get("id"))


if __name__ == "__main__":
    main()
