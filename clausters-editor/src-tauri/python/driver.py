"""Driver between the editor and a persistent Python session.

Protocol: one JSON request per line on stdin, one JSON message per line on the
protocol descriptor (the original stdout). fd 1 is redirected to stderr so that
no outside output (C, subprocesses) corrupts the protocol.

Requests:  {"id": 1, "op": "eval", "code": "...", "file": "a.py", "line": 10}
           {"id": 2, "op": "help", "expr": "print"}
           {"id": 3, "op": "inspect" | "signatures" | "complete",
            "source": "the editor's text", "line": 1, "column": 4}
Messages:  {"type": "out" | "err" | "result" | "help" | "done", "id": 1, "text": "..."}
           {"type": "inspect" | "signatures" | "complete", "id": 3, "data": ...}

Evaluations run on the main thread, one at a time. "help", "inspect",
"signatures" and "complete" are answered by the reader thread right away, so the
documentation and the code intelligence respond even while code is running.
"""

import ast
import builtins
import io
import json
import linecache
import os
import pydoc
import queue
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


# --- Code intelligence: Jedi over the editor's text and the session's namespace ---
#
# Completions, call signatures and hover help come from Jedi's Interpreter, which
# reads the source being edited together with the live objects of the session --
# so it knows what a `from module import *` brought in, and what `Server().boot` or
# `sine(440).midicps` is. Jedi is a dependency of the clausters-editor package, and
# the environments the editor creates install it; in an environment without it the
# editor still runs code, without these.

try:
    import jedi
except ImportError:
    jedi = None

_MAX_DOC = 4000
_MAX_COMPLETIONS = 500
_jedi_missing_said = False


def _script(req):
    global _jedi_missing_said
    if jedi is None:
        if not _jedi_missing_said:
            _jedi_missing_said = True
            emit("info", None, "Completions, signatures and hover help need jedi in this "
                               "environment: pip install jedi\n")
        return None
    return jedi.Interpreter(req.get("source", ""), [namespace])


def _at(req):
    """Jedi's position: a 1-based line and a 0-based column."""
    return int(req.get("line", 1)), int(req.get("column", 0))


def describe(req):
    """What the name at the position is, for the hover tooltip and for help: its
    names, kind, signature and docstring. None if there is no name there."""
    script = _script(req)
    if script is None:
        return None
    try:
        names = script.infer(*_at(req)) or script.help(*_at(req))
    except Exception:
        return None
    names = [n for n in names if n.type != "keyword"]
    if not names:
        return None
    n = names[0]
    full = n.full_name or n.name
    module = n.module_name or ""
    qualname = full[len(module) + 1:] if module and full.startswith(module + ".") else full
    try:
        sigs = n.get_signatures()
    except Exception:
        sigs = []
    doc = n.docstring(raw=True) or ""
    return {
        "name": n.name,
        "qualname": qualname,
        "fullname": full,
        "kind": n.type,
        "signature": sigs[0].to_string() if sigs else "",
        "params": [p.to_string() for p in sigs[0].params] if sigs else None,
        "doc": doc if len(doc) <= _MAX_DOC else doc[:_MAX_DOC] + "\n...",
    }


def signatures(req):
    """The signatures of the call the position is in, with the argument it is at."""
    script = _script(req)
    if script is None:
        return []
    try:
        sigs = script.get_signatures(*_at(req))
    except Exception:
        return []
    out = []
    for s in sigs:
        doc = s.docstring(raw=True) or ""
        out.append({
            "name": s.name,
            "params": [p.to_string() for p in s.params],
            "index": s.index,
            "bracket": list(s.bracket_start),
            "doc": doc.split("\n\n")[0],
        })
    return out


def complete(req):
    """The completions at the position: each name, its kind, and how many of its
    characters are already typed."""
    script = _script(req)
    if script is None:
        return []
    try:
        found = script.complete(*_at(req))
    except Exception:
        return []
    return [
        {"name": c.name, "kind": c.type, "typed": len(c.name) - len(c.complete)}
        for c in found[:_MAX_COMPLETIONS]
    ]


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
            emit("inspect", req.get("id"), data=describe(req))
        elif op == "signatures":
            emit("signatures", req.get("id"), data=signatures(req))
        elif op == "complete":
            emit("complete", req.get("id"), data=complete(req))
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
