"""Operator-overloading base (port of ``sc3/base/absobject.py``).

``AbstractObject`` overloads Python's arithmetic, comparison and bitwise
operators and exposes named math methods, routing every one through four
dispatch hooks a subclass implements:

- ``_compose_unop(selector)``           — unary
- ``_compose_binop(selector, other)``   — binary
- ``_rcompose_binop(selector, other)``  — reflected binary (``other <op> self``)
- ``_compose_narop(selector, *args)``   — n-ary

The ``selector`` strings are the same operator names used by
`clausters.base.builtins` (value side) and by
`clausters.defs.signals` (graph side) — so the *same* expression composes
either concrete numbers or a Faust/UGen graph depending on the subclass. This
is what lets one piece of code describe both.
"""


class AbstractObject:
    # --- dispatch hooks (subclass responsibility) ---

    def _compose_unop(self, selector):
        raise NotImplementedError(f"{type(self).__name__}._compose_unop")

    def _compose_binop(self, selector, other):
        raise NotImplementedError(f"{type(self).__name__}._compose_binop")

    def _rcompose_binop(self, selector, other):
        raise NotImplementedError(f"{type(self).__name__}._rcompose_binop")

    def _compose_narop(self, selector, *args):
        raise NotImplementedError(f"{type(self).__name__}._compose_narop")

    # --- unary operators ---

    def __neg__(self): return self._compose_unop("neg")
    def __pos__(self): return self
    def __abs__(self): return self._compose_unop("abs")
    def __floor__(self): return self._compose_unop("floor")
    def __ceil__(self): return self._compose_unop("ceil")

    # --- binary operators ---

    def __add__(self, other): return self._compose_binop("add", other)
    def __radd__(self, other): return self._rcompose_binop("add", other)
    def __sub__(self, other): return self._compose_binop("sub", other)
    def __rsub__(self, other): return self._rcompose_binop("sub", other)
    def __mul__(self, other): return self._compose_binop("mul", other)
    def __rmul__(self, other): return self._rcompose_binop("mul", other)
    def __truediv__(self, other): return self._compose_binop("div", other)
    def __rtruediv__(self, other): return self._rcompose_binop("div", other)
    def __mod__(self, other): return self._compose_binop("mod", other)
    def __rmod__(self, other): return self._rcompose_binop("mod", other)
    def __pow__(self, other): return self._compose_binop("pow", other)
    def __rpow__(self, other): return self._rcompose_binop("pow", other)
    def __lshift__(self, other): return self._compose_binop("lshift", other)
    def __rlshift__(self, other): return self._rcompose_binop("lshift", other)
    def __rshift__(self, other): return self._compose_binop("rshift", other)
    def __rrshift__(self, other): return self._rcompose_binop("rshift", other)
    def __and__(self, other): return self._compose_binop("bitand", other)
    def __rand__(self, other): return self._rcompose_binop("bitand", other)
    def __or__(self, other): return self._compose_binop("bitor", other)
    def __ror__(self, other): return self._rcompose_binop("bitor", other)
    def __xor__(self, other): return self._compose_binop("bitxor", other)
    def __rxor__(self, other): return self._rcompose_binop("bitxor", other)

    # --- comparison operators (return composed objects, not bools) ---

    def __lt__(self, other): return self._compose_binop("lt", other)
    def __le__(self, other): return self._compose_binop("le", other)
    def __gt__(self, other): return self._compose_binop("gt", other)
    def __ge__(self, other): return self._compose_binop("ge", other)

    # --- named unary methods ---

    def abs(self): return self._compose_unop("abs")
    def neg(self): return self._compose_unop("neg")
    def floor(self): return self._compose_unop("floor")
    def ceil(self): return self._compose_unop("ceil")
    def sin(self): return self._compose_unop("sin")
    def cos(self): return self._compose_unop("cos")
    def tan(self): return self._compose_unop("tan")
    def asin(self): return self._compose_unop("asin")
    def acos(self): return self._compose_unop("acos")
    def atan(self): return self._compose_unop("atan")
    def exp(self): return self._compose_unop("exp")
    def log(self): return self._compose_unop("log")
    def log10(self): return self._compose_unop("log10")
    def sqrt(self): return self._compose_unop("sqrt")
    def asinteger(self): return self._compose_unop("as_int")
    def asfloat(self): return self._compose_unop("as_float")
    def log2(self): return self._compose_unop("log2")
    def squared(self): return self._compose_unop("squared")
    def cubed(self): return self._compose_unop("cubed")
    def reciprocal(self): return self._compose_unop("recip")
    def frac(self): return self._compose_unop("frac")
    def sign(self): return self._compose_unop("sign")
    def sinh(self): return self._compose_unop("sinh")
    def cosh(self): return self._compose_unop("cosh")
    def tanh(self): return self._compose_unop("tanh")
    def distort(self): return self._compose_unop("distort")
    def softclip(self): return self._compose_unop("softclip")
    def midicps(self): return self._compose_unop("midicps")
    def cpsmidi(self): return self._compose_unop("cpsmidi")
    def midiratio(self): return self._compose_unop("midiratio")
    def ratiomidi(self): return self._compose_unop("ratiomidi")
    def dbamp(self): return self._compose_unop("dbamp")
    def ampdb(self): return self._compose_unop("ampdb")
    def octcps(self): return self._compose_unop("octcps")
    def cpsoct(self): return self._compose_unop("cpsoct")

    # --- named binary methods ---

    def min(self, other): return self._compose_binop("min", other)
    def max(self, other): return self._compose_binop("max", other)
    def atan2(self, other): return self._compose_binop("atan2", other)
    def pow(self, other): return self._compose_binop("pow", other)
    def mod(self, other): return self._compose_binop("mod", other)
    def hypot(self, other): return self._compose_binop("hypot", other)
    def hypotapx(self, other): return self._compose_binop("hypot_apx", other)
    def clip2(self, other): return self._compose_binop("clip2", other)
    def fold2(self, other): return self._compose_binop("fold2", other)
    def wrap2(self, other): return self._compose_binop("wrap2", other)
    def gcd(self, other): return self._compose_binop("gcd", other)
    def lcm(self, other): return self._compose_binop("lcm", other)
    def excess(self, other): return self._compose_binop("excess", other)
    def absdif(self, other): return self._compose_binop("absdif", other)
    def thresh(self, other): return self._compose_binop("thresh", other)
    def round(self, other): return self._compose_binop("round", other)
    def trunc(self, other): return self._compose_binop("trunc", other)
    def ring1(self, other): return self._compose_binop("ring1", other)
    def ring2(self, other): return self._compose_binop("ring2", other)
    def ring3(self, other): return self._compose_binop("ring3", other)
    def ring4(self, other): return self._compose_binop("ring4", other)
    def sumsqr(self, other): return self._compose_binop("sumsqr", other)
    def difsqr(self, other): return self._compose_binop("difsqr", other)
    def sqrsum(self, other): return self._compose_binop("sqrsum", other)
    def sqrdif(self, other): return self._compose_binop("sqrdif", other)
