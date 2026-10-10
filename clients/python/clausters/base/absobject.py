"""Operator-overloading base (port of ``sc3/base/absobject.py``).

``AbstractObject`` overloads Python's arithmetic, comparison and bitwise
operators and exposes named math methods, routing every one through four
dispatch hooks a subclass implements:

- ``_compose_unop(selector)``           -- unary
- ``_compose_binop(selector, other)``   -- binary
- ``_rcompose_binop(selector, other)``  -- reflected binary (``other <op> self``)
- ``_compose_narop(selector, *args, **static)`` -- n-ary: the range maps,
  the two-bound operators, the random draws and sclang's value functions
  (``blend``, ``moddif`` ...); a subclass with no such operator refuses it by
  name (a Faust signal has no ``linlin``, a signal no ``moddif``)

The ``selector`` strings are the same operator names used by
`clausters.base.builtins` (value side) and by
`clausters.defs.signals` (graph side) -- so the *same* expression composes
values (a pattern, a stream), a UGen or Faust graph, or a per-bin program
depending on the subclass. This is what lets one piece of code describe them
all. Equality is not among the operators: ``==`` stays Python's, so a pattern
or a node still hashes and is found with ``in``; the composed comparison is
the named `eq`/`ne`.
"""


class AbstractObject:
    # --- dispatch hooks (subclass responsibility) ---

    def _compose_unop(self, selector):
        raise NotImplementedError(f"{type(self).__name__}._compose_unop")

    def _compose_binop(self, selector, other):
        raise NotImplementedError(f"{type(self).__name__}._compose_binop")

    def _rcompose_binop(self, selector, other):
        raise NotImplementedError(f"{type(self).__name__}._rcompose_binop")

    def _compose_narop(self, selector, *args, **static):
        raise TypeError(f"a {type(self).__name__} has no {selector!r}")

    # --- unary operators ---

    def __neg__(self): return self._compose_unop("neg")
    def __pos__(self): return self
    def __abs__(self): return self._compose_unop("abs")
    def __floor__(self): return self._compose_unop("floor")
    def __ceil__(self): return self._compose_unop("ceil")
    def __invert__(self): return self._compose_unop("bitnot")

    # --- binary operators ---

    def __add__(self, other): return self._compose_binop("add", other)
    def __radd__(self, other): return self._rcompose_binop("add", other)
    def __sub__(self, other): return self._compose_binop("sub", other)
    def __rsub__(self, other): return self._rcompose_binop("sub", other)
    def __mul__(self, other): return self._compose_binop("mul", other)
    def __rmul__(self, other): return self._rcompose_binop("mul", other)
    def __truediv__(self, other): return self._compose_binop("div", other)
    def __rtruediv__(self, other): return self._rcompose_binop("div", other)
    def __floordiv__(self, other): return self._compose_binop("idiv", other)
    def __rfloordiv__(self, other): return self._rcompose_binop("idiv", other)
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
    def exp10(self): return self._compose_unop("exp10")
    def log(self): return self._compose_unop("log")
    def log10(self): return self._compose_unop("log10")
    def sqrt(self): return self._compose_unop("sqrt")
    def asinteger(self): return self._compose_unop("asint")
    def asfloat(self): return self._compose_unop("asfloat")
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
    def bitnot(self): return self._compose_unop("bitnot")
    def rectwindow(self): return self._compose_unop("rectwindow")
    def hanwindow(self): return self._compose_unop("hanwindow")
    def welwindow(self): return self._compose_unop("welwindow")
    def triwindow(self): return self._compose_unop("triwindow")
    def ramp(self): return self._compose_unop("ramp")
    def scurve(self): return self._compose_unop("scurve")
    def rint(self): return self._compose_unop("rint")
    def not_(self):
        """``1`` where this is not above zero, else ``0`` -- sclang's ``not``
        (``not`` itself is Python's keyword)."""
        return self._compose_binop("le", 0)

    # --- named binary methods ---

    def min(self, other): return self._compose_binop("min", other)
    def max(self, other): return self._compose_binop("max", other)
    def atan2(self, other): return self._compose_binop("atan2", other)
    def pow(self, other): return self._compose_binop("pow", other)
    def mod(self, other): return self._compose_binop("mod", other)
    def hypot(self, other): return self._compose_binop("hypot", other)
    def hypotapx(self, other): return self._compose_binop("hypotapx", other)
    def clip2(self, other): return self._compose_binop("clip2", other)
    def fold2(self, other): return self._compose_binop("fold2", other)
    def wrap2(self, other): return self._compose_binop("wrap2", other)
    def idiv(self, other): return self._compose_binop("idiv", other)
    def roundup(self, other): return self._compose_binop("roundup", other)
    def urshift(self, other): return self._compose_binop("urshift", other)
    def amclip(self, other): return self._compose_binop("amclip", other)
    def scaleneg(self, other): return self._compose_binop("scaleneg", other)
    def firstarg(self, other): return self._compose_binop("firstarg", other)
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
    def eq(self, other): return self._compose_binop("eq", other)
    def ne(self, other): return self._compose_binop("ne", other)
    def bitand(self, other): return self._compose_binop("bitand", other)
    def bitor(self, other): return self._compose_binop("bitor", other)
    def bitxor(self, other): return self._compose_binop("bitxor", other)
    def leftshift(self, other): return self._compose_binop("lshift", other)
    def rightshift(self, other): return self._compose_binop("rshift", other)

    # --- n-ary: through `_compose_narop`, whose selector names the function ---

    def linlin(self, in_lo, in_hi, out_lo, out_hi, clip="minmax"):
        """This off a linear range onto a linear one."""
        return self._compose_narop(
            "linlin", in_lo, in_hi, out_lo, out_hi, clip=clip)

    def linexp(self, in_lo, in_hi, out_lo, out_hi, clip="minmax"):
        """Off a linear range onto an exponential one -- an LFO onto a
        frequency. The output ends must not straddle zero; one *at* zero is
        nudged to the smallest same-signed value rather than giving a NaN."""
        return self._compose_narop(
            "linexp", in_lo, in_hi, out_lo, out_hi, clip=clip)

    def explin(self, in_lo, in_hi, out_lo, out_hi, clip="minmax"):
        """Off an exponential range onto a linear one -- a frequency onto a
        fader position."""
        return self._compose_narop(
            "explin", in_lo, in_hi, out_lo, out_hi, clip=clip)

    def expexp(self, in_lo, in_hi, out_lo, out_hi, clip="minmax"):
        """Off an exponential range onto another."""
        return self._compose_narop(
            "expexp", in_lo, in_hi, out_lo, out_hi, clip=clip)

    def lincurve(self, in_lo, in_hi, out_lo, out_hi, curve=-4.0,
                 clip="minmax"):
        """Off a linear range onto one **bent** by ``curve``: 0 is linear,
        negative builds fast then slow -- most of the output spent on the first
        half of the input -- and positive the reverse, which is the
        fine-at-the-bottom feel a frequency or an amplitude control wants.
        Unlike `linexp` the bend spans zero freely."""
        return self._compose_narop(
            "lincurve", in_lo, in_hi, out_lo, out_hi, curve, clip=clip)

    def curvelin(self, in_lo, in_hi, out_lo, out_hi, curve=-4.0,
                 clip="minmax"):
        """The inverse of `lincurve`: off a bent range onto a linear one."""
        return self._compose_narop(
            "curvelin", in_lo, in_hi, out_lo, out_hi, curve, clip=clip)

    # ---- two bounds: the general form of clip2/wrap2/fold2 (on a signal,
    # the Clip/Wrap/Fold UGen of the same name).

    def clip(self, lo, hi):
        """This held inside ``lo..hi`` at the nearer bound."""
        return self._compose_narop("clip", lo, hi)

    def wrap(self, lo, hi):
        """This wrapped into ``lo..hi`` -- taken modulo the range."""
        return self._compose_narop("wrap", lo, hi)

    def fold(self, lo, hi):
        """This folded into ``lo..hi`` -- reflected at the bounds."""
        return self._compose_narop("fold", lo, hi)

    # ---- random draws: on a value or a pattern, one per value from the
    # random context; on a signal, a new value every sample, each node its own
    # seeded stream (sclang's random operators).

    def rand(self):
        """A uniform draw in ``[0, self)``."""
        return self._compose_narop("rand")

    def rand2(self):
        """A uniform draw in ``[-self, self)``."""
        return self._compose_narop("rand2")

    def linrand(self):
        """A draw in ``[0, self)``, more likely toward 0."""
        return self._compose_narop("linrand")

    def bilinrand(self):
        """A draw in ``(-self, self)``, more likely toward 0."""
        return self._compose_narop("bilinrand")

    def sum3rand(self):
        """A bell-shaped draw in ``(-self, self)``."""
        return self._compose_narop("sum3rand")

    def coin(self):
        """``1`` with probability ``self``, else ``0``."""
        return self._compose_narop("coin")

    def rrand(self, hi):
        """A uniform draw between this and ``hi``."""
        return self._compose_narop("rrand", hi)

    def exprand(self, hi):
        """An exponential draw between this and ``hi``."""
        return self._compose_narop("exprand", hi)

    # ---- the value functions sclang's AbstractFunction has beside them.

    def blend(self, other, frac=0.5):
        """``frac`` of the way from this to ``other``."""
        return self._compose_narop("blend", other, frac)

    def degrad(self):
        """Degrees to radians."""
        return self._compose_narop("degrad")

    def raddeg(self):
        """Radians to degrees."""
        return self._compose_narop("raddeg")

    def moddif(self, other=0.0, mod=1.0):
        """The smaller distance between this and ``other`` around a circle of
        ``mod``."""
        return self._compose_narop("moddif", other, mod)

    def bilin(self, in_center, in_lo, in_hi, out_center, out_lo, out_hi,
              clip="minmax"):
        """Off a range split at ``in_center`` onto one split at
        ``out_center``, each half mapped linearly."""
        return self._compose_narop("bilin", in_center, in_lo, in_hi,
                                   out_center, out_lo, out_hi, clip=clip)

    def biexp(self, in_center, in_lo, in_hi, out_center, out_lo, out_hi,
              clip="minmax"):
        """As `bilin`, with each half read exponentially."""
        return self._compose_narop("biexp", in_center, in_lo, in_hi,
                                   out_center, out_lo, out_hi, clip=clip)
