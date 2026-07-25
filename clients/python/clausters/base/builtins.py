"""Numeric builtins on scalars and lists, dispatched to the native core.

Port of the value side of ``sc3/base/builtins.py``: the operations the client
applies to concrete numbers. The arithmetic/comparison/transcendental
primitives go through `clausters._native` (the shared ``clausters-core``)
so they are computed in **f32**, matching the server by construction — Python's
own ``float`` is f64 and would diverge. The music-theory conversions
(``midicps`` …) go through the core too, so they are bit-identical to the
server's ``UnaryOpUGen`` (S3) — a value computed off the RT path and the same op
on the audio thread agree exactly.

Each function accepts a scalar or a list/tuple. With a list it returns a list,
extending the shorter operand cyclically (sc3 semantics). The boundary stays
flat: numbers in, numbers (or a list of them) out.
"""

import builtins as _py

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
def hypot(a, b): return _binop(BinaryOp.HYPOT, a, b)
def ring1(a, b): return _binop(BinaryOp.RING1, a, b)
def ring2(a, b): return _binop(BinaryOp.RING2, a, b)
def ring3(a, b): return _binop(BinaryOp.RING3, a, b)
def ring4(a, b): return _binop(BinaryOp.RING4, a, b)
def sumsqr(a, b): return _binop(BinaryOp.SUMSQR, a, b)
def difsqr(a, b): return _binop(BinaryOp.DIFSQR, a, b)
def sqrsum(a, b): return _binop(BinaryOp.SQRSUM, a, b)
def sqrdif(a, b): return _binop(BinaryOp.SQRDIF, a, b)
def absdif(a, b): return _binop(BinaryOp.ABSDIF, a, b)
def thresh(a, b): return _binop(BinaryOp.THRESH, a, b)
def clip2(a, b): return _binop(BinaryOp.CLIP2, a, b)
def excess(a, b): return _binop(BinaryOp.EXCESS, a, b)
def round(a, b): return _binop(BinaryOp.ROUND, a, b)
def trunc(a, b): return _binop(BinaryOp.TRUNC, a, b)
def fold2(a, b): return _binop(BinaryOp.FOLD2, a, b)
def wrap2(a, b): return _binop(BinaryOp.WRAP2, a, b)
def gcd(a, b): return _binop(BinaryOp.GCD, a, b)
def lcm(a, b): return _binop(BinaryOp.LCM, a, b)
def hypot_apx(a, b): return _binop(BinaryOp.HYPOT_APX, a, b)


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
def squared(x): return _unop(UnaryOp.SQUARED, x)
def cubed(x): return _unop(UnaryOp.CUBED, x)
def recip(x): return _unop(UnaryOp.RECIP, x)
def frac(x): return _unop(UnaryOp.FRAC, x)
def sign(x): return _unop(UnaryOp.SIGN, x)
def log2(x): return _unop(UnaryOp.LOG2, x)
def sinh(x): return _unop(UnaryOp.SINH, x)
def cosh(x): return _unop(UnaryOp.COSH, x)
def tanh(x): return _unop(UnaryOp.TANH, x)
def distort(x): return _unop(UnaryOp.DISTORT, x)
def softclip(x): return _unop(UnaryOp.SOFTCLIP, x)


# ---- music-theory conversions (native, f32 — bit-identical to the server) ----

def midicps(x): return _unop(UnaryOp.MIDICPS, x)
def cpsmidi(x): return _unop(UnaryOp.CPSMIDI, x)
def midiratio(x): return _unop(UnaryOp.MIDIRATIO, x)
def ratiomidi(x): return _unop(UnaryOp.RATIOMIDI, x)
def dbamp(x): return _unop(UnaryOp.DBAMP, x)
def ampdb(x): return _unop(UnaryOp.AMPDB, x)
def octcps(x): return _unop(UnaryOp.OCTCPS, x)
def cpsoct(x): return _unop(UnaryOp.CPSOCT, x)


# Selector → function, so AbstractObject value subclasses can dispatch by the
# same operator names the graph layer uses.
UNARY = {
    "neg": neg, "abs": abso, "sin": sin, "cos": cos, "tan": tan, "asin": asin,
    "acos": acos, "atan": atan, "exp": exp, "log": log, "log10": log10,
    "log2": log2, "sqrt": sqrt, "floor": floor, "ceil": ceil, "rint": rint,
    "as_int": as_int, "as_float": as_float, "squared": squared, "cubed": cubed,
    "recip": recip, "frac": frac, "sign": sign, "sinh": sinh, "cosh": cosh,
    "tanh": tanh, "distort": distort, "softclip": softclip, "midicps": midicps,
    "cpsmidi": cpsmidi, "midiratio": midiratio, "ratiomidi": ratiomidi,
    "dbamp": dbamp, "ampdb": ampdb, "octcps": octcps, "cpsoct": cpsoct,
}
BINARY = {
    "add": add, "sub": sub, "mul": mul, "div": div, "mod": mod, "pow": pow,
    "min": min, "max": max, "atan2": atan2, "gt": gt, "lt": lt, "ge": ge,
    "le": le, "eq": eq, "ne": ne, "bitand": bitand, "bitor": bitor,
    "bitxor": bitxor, "lshift": lshift, "rshift": rshift, "hypot": hypot,
    "ring1": ring1, "ring2": ring2, "ring3": ring3, "ring4": ring4,
    "sumsqr": sumsqr, "difsqr": difsqr, "sqrsum": sqrsum, "sqrdif": sqrdif,
    "absdif": absdif, "thresh": thresh, "clip2": clip2, "excess": excess,
    "round": round, "trunc": trunc, "fold2": fold2, "wrap2": wrap2,
    "gcd": gcd, "lcm": lcm, "hypot_apx": hypot_apx,
}
