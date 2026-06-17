"""Numeric builtins on scalars and lists, dispatched to the native core.

Port of the value side of ``sc3/base/builtins.py``: the operations the client
applies to concrete numbers. The arithmetic/comparison/transcendental
primitives go through :mod:`clausters._native` (the shared ``clausters-core``)
so they are computed in **f32**, matching the server by construction — Python's
own ``float`` is f64 and would diverge. Music-theory helpers that the server
only reaches through Faust composition (``midicps`` …) are pure Python with the
standard formula.

Each function accepts a scalar or a list/tuple. With a list it returns a list,
extending the shorter operand cyclically (sc3 semantics). The boundary stays
flat: numbers in, numbers (or a list of them) out.
"""

import builtins as _py
import math

from .. import _native

BinaryOp = _native.BinaryOp
UnaryOp = _native.UnaryOp


def _is_seq(x):
    return isinstance(x, (list, tuple))


def _extend(seq, n):
    length = len(seq)
    return [seq[i % length] for i in range(n)]


def _unop(op, x):
    if _is_seq(x):
        return list(_native.unary(op, [float(v) for v in x]))
    return _native.unary(op, float(x))


def _binop(op, a, b):
    a_seq, b_seq = _is_seq(a), _is_seq(b)
    if not a_seq and not b_seq:
        return _native.binary(op, float(a), float(b))
    if a_seq and b_seq:
        n = _py.max(len(a), len(b))
        a, b = _extend(a, n), _extend(b, n)
        return list(_native.binary(op, [float(v) for v in a], [float(v) for v in b]))
    # one scalar, one list: the core broadcasts the length-1 operand
    if a_seq:
        return list(_native.binary(op, [float(v) for v in a], float(b)))
    return list(_native.binary(op, float(a), [float(v) for v in b]))


# ---- binary primitives (native, f32) ----

def add(a, b): return _binop(BinaryOp.ADD, a, b)
def sub(a, b): return _binop(BinaryOp.SUB, a, b)
def mul(a, b): return _binop(BinaryOp.MUL, a, b)
def div(a, b): return _binop(BinaryOp.DIV, a, b)
def mod(a, b): return _binop(BinaryOp.MOD, a, b)
def pow(a, b): return _binop(BinaryOp.POW, a, b)
def min(a, b): return _binop(BinaryOp.MIN, a, b)
def max(a, b): return _binop(BinaryOp.MAX, a, b)
def atan2(a, b): return _binop(BinaryOp.ATAN2, a, b)
def gt(a, b): return _binop(BinaryOp.GT, a, b)
def lt(a, b): return _binop(BinaryOp.LT, a, b)
def ge(a, b): return _binop(BinaryOp.GE, a, b)
def le(a, b): return _binop(BinaryOp.LE, a, b)
def eq(a, b): return _binop(BinaryOp.EQ, a, b)
def ne(a, b): return _binop(BinaryOp.NE, a, b)
def bitand(a, b): return _binop(BinaryOp.AND, a, b)
def bitor(a, b): return _binop(BinaryOp.OR, a, b)
def bitxor(a, b): return _binop(BinaryOp.XOR, a, b)
def lshift(a, b): return _binop(BinaryOp.LSH, a, b)
def rshift(a, b): return _binop(BinaryOp.RSH, a, b)


# ---- unary primitives (native, f32) ----

def neg(x): return _unop(UnaryOp.NEG, x)
def abso(x): return _unop(UnaryOp.ABS, x)
def sin(x): return _unop(UnaryOp.SIN, x)
def cos(x): return _unop(UnaryOp.COS, x)
def tan(x): return _unop(UnaryOp.TAN, x)
def asin(x): return _unop(UnaryOp.ASIN, x)
def acos(x): return _unop(UnaryOp.ACOS, x)
def atan(x): return _unop(UnaryOp.ATAN, x)
def exp(x): return _unop(UnaryOp.EXP, x)
def log(x): return _unop(UnaryOp.LOG, x)
def log10(x): return _unop(UnaryOp.LOG10, x)
def sqrt(x): return _unop(UnaryOp.SQRT, x)
def floor(x): return _unop(UnaryOp.FLOOR, x)
def ceil(x): return _unop(UnaryOp.CEIL, x)
def rint(x): return _unop(UnaryOp.RINT, x)
def as_int(x): return _unop(UnaryOp.INTCAST, x)
def as_float(x): return _unop(UnaryOp.FLOATCAST, x)


# ---- music-theory helpers (pure Python, standard formulas) ----

def _elementwise(fn, x):
    return [fn(float(v)) for v in x] if _is_seq(x) else fn(float(x))


def midicps(x): return _elementwise(lambda m: 440.0 * 2.0 ** ((m - 69.0) / 12.0), x)
def cpsmidi(x): return _elementwise(lambda f: 69.0 + 12.0 * math.log2(f / 440.0), x)
def midiratio(x): return _elementwise(lambda i: 2.0 ** (i / 12.0), x)
def ratiomidi(x): return _elementwise(lambda r: 12.0 * math.log2(r), x)
def dbamp(x): return _elementwise(lambda db: 10.0 ** (db / 20.0), x)
def ampdb(x): return _elementwise(lambda a: 20.0 * math.log10(a), x)


# Selector → function, so AbstractObject value subclasses can dispatch by the
# same operator names the graph layer uses.
UNARY = {
    "neg": neg, "abs": abso, "sin": sin, "cos": cos, "tan": tan, "asin": asin,
    "acos": acos, "atan": atan, "exp": exp, "log": log, "log10": log10,
    "sqrt": sqrt, "floor": floor, "ceil": ceil, "rint": rint,
    "as_int": as_int, "as_float": as_float, "midicps": midicps,
    "cpsmidi": cpsmidi, "midiratio": midiratio, "ratiomidi": ratiomidi,
    "dbamp": dbamp, "ampdb": ampdb,
}
BINARY = {
    "add": add, "sub": sub, "mul": mul, "div": div, "mod": mod, "pow": pow,
    "min": min, "max": max, "atan2": atan2, "gt": gt, "lt": lt, "ge": ge,
    "le": le, "eq": eq, "ne": ne, "bitand": bitand, "bitor": bitor,
    "bitxor": bitxor, "lshift": lshift, "rshift": rshift,
}
