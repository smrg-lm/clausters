"""Numeric builtins on scalars and lists, dispatched to the native core.

Port of the value side of ``sc3/base/builtins.py``: the operations the client
applies to concrete numbers. The arithmetic/comparison/transcendental
primitives go through `clausters._native` (the shared ``clausters-core``)
so they are computed in **f32**, matching the server by construction -- Python's
own ``float`` is f64 and would diverge. The music-theory conversions
(``midicps`` ...) go through the core too, so they are bit-identical to the
server's ``UnaryOpUGen`` (S3) -- a value computed off the RT path and the same op
on the audio thread agree exactly.

Each function accepts a scalar or a list/tuple. With a list it returns a list,
extending the shorter operand cyclically (sc3 semantics). The boundary stays
flat: numbers in, numbers (or a list of them) out.

The families, in the order below: the **operators** (the server's op tables,
``f32``; where C and scsynth disagree on what one means -- a signed ``sqrt``
and ``pow``, a floored ``mod`` -- it is scsynth's); ``clip``/``wrap``/``fold``
between **two bounds**; the **perceptual scales** (``cpsmel`` ...); sclang's
**functions that are not operators** (``degrad``, ``snap``, ``bilin``, ``erf``
...) and the ones over a **scale** (``degkey``, ``nearestinscale`` ...) and over
a **whole list** (``blend``, ``normalize``, ``resamp1``, ``asrandomtable`` ...),
all three in ``f64`` as sclang computes them; and the **range maps**. The draws
-- ``rand``, ``exprand``, ``choose``, ``tablerand`` ... -- are random values and
live in `clausters.base.rand`.
"""

import builtins as _py

from .. import _native

BinaryOp = _native.BinaryOp
UnaryOp = _native.UnaryOp
BoundOp = _native.BoundOp
MapOp = _native.MapOp
Clip = _native.Clip


def _is_seq(x):
    """Whether `x` is a sequence of numbers rather than one number.

    Anything iterable that is not a string counts, so the idiomatic Python
    spellings all work -- `midicps(range(0, 120))`, a generator, a `Buffer`'s
    samples -- rather than only the two types a first pass happened to name.
    """
    return not isinstance(x, (str, bytes)) and hasattr(x, "__iter__")


def _extend(seq, n):
    seq = list(seq)
    length = len(seq)
    return [seq[i % length] for i in _py.range(n)]


def _unop(op, x):
    if _is_seq(x):
        return list(_native.unary(op, [float(v) for v in x]))
    return _native.unary(op, float(x))


def _scale(fn, x):
    """One f64 core conversion over a scalar or a sequence.

    Not `_unop`: these are plain core functions rather than entries in the
    unary-op table, so there is no batched call to make and the loop is here.
    """
    if _is_seq(x):
        return [fn(float(v)) for v in x]
    return fn(float(x))


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


def _boundop(op, x, lo, hi):
    args = (x, lo, hi)
    seqs = [_is_seq(v) for v in args]
    if not _py.any(seqs):
        return _native.bound(op, float(x), float(lo), float(hi))
    n = _py.max(len(v) for v, q in zip(args, seqs) if q)
    # A sequence extends cyclically (sc3's rule); a scalar is a constant the
    # core broadcasts.
    x, lo, hi = (
        [float(e) for e in _extend(v, n)] if q else float(v)
        for v, q in zip(args, seqs)
    )
    return list(_native.bound(op, x, lo, hi))


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
def leftshift(a, b): return _binop(BinaryOp.LSH, a, b)
def rightshift(a, b): return _binop(BinaryOp.RSH, a, b)
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
def hypotapx(a, b): return _binop(BinaryOp.HYPOT_APX, a, b)
def idiv(a, b): return _binop(BinaryOp.IDIV, a, b)
def roundup(a, b): return _binop(BinaryOp.ROUNDUP, a, b)
def urshift(a, b): return _binop(BinaryOp.URSHIFT, a, b)
def amclip(a, b): return _binop(BinaryOp.AMCLIP, a, b)
def scaleneg(a, b): return _binop(BinaryOp.SCALENEG, a, b)
def firstarg(a, b): return _binop(BinaryOp.FIRSTARG, a, b)


# ---- two bounds (native, f32) ----
#
# The general form of `clip2`/`wrap2`/`fold2`, whose range is the symmetric
# ``-b..b``: the value kept inside ``lo..hi``. The server's ``Clip``/``Wrap``/
# ``Fold`` UGens are the same function over a signal.

def clip(x, lo, hi):
    """`x` held inside ``lo..hi`` at the nearer bound."""
    return _boundop(BoundOp.CLIP, x, lo, hi)


def wrap(x, lo, hi):
    """`x` wrapped into ``lo..hi`` -- taken modulo the range."""
    return _boundop(BoundOp.WRAP, x, lo, hi)


def fold(x, lo, hi):
    """`x` folded into ``lo..hi`` -- reflected at the bounds, as many times
    as it takes."""
    return _boundop(BoundOp.FOLD, x, lo, hi)


# ---- unary primitives (native, f32) ----

def neg(x): return _unop(UnaryOp.NEG, x)
def abs(x): return _unop(UnaryOp.ABS, x)
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
def asinteger(x): return _unop(UnaryOp.INTCAST, x)
def asfloat(x): return _unop(UnaryOp.FLOATCAST, x)
def squared(x): return _unop(UnaryOp.SQUARED, x)
def cubed(x): return _unop(UnaryOp.CUBED, x)
def reciprocal(x): return _unop(UnaryOp.RECIP, x)
def frac(x): return _unop(UnaryOp.FRAC, x)
def sign(x): return _unop(UnaryOp.SIGN, x)
def log2(x): return _unop(UnaryOp.LOG2, x)
def exp10(x): return _unop(UnaryOp.EXP10, x)
def sinh(x): return _unop(UnaryOp.SINH, x)
def cosh(x): return _unop(UnaryOp.COSH, x)
def tanh(x): return _unop(UnaryOp.TANH, x)
def distort(x): return _unop(UnaryOp.DISTORT, x)
def softclip(x): return _unop(UnaryOp.SOFTCLIP, x)
def bitnot(x): return _unop(UnaryOp.BITNOT, x)
def ramp(x): return _unop(UnaryOp.RAMP, x)
def scurve(x): return _unop(UnaryOp.SCURVE, x)


# ---- windows over a phase (native, f32) ----
#
# A window's shape read at one phase in ``0..1``, zero outside it -- the
# operator, not an FFT frame (`clausters._native.window` fills one of those).

def rectwindow(x): return _unop(UnaryOp.RECTWINDOW, x)
def hanwindow(x): return _unop(UnaryOp.HANWINDOW, x)
def welwindow(x): return _unop(UnaryOp.WELWINDOW, x)
def triwindow(x): return _unop(UnaryOp.TRIWINDOW, x)


# ---- music-theory conversions (native, f32 -- bit-identical to the server) ----

def midicps(x): return _unop(UnaryOp.MIDICPS, x)
def cpsmidi(x): return _unop(UnaryOp.CPSMIDI, x)
def midiratio(x): return _unop(UnaryOp.MIDIRATIO, x)
def ratiomidi(x): return _unop(UnaryOp.RATIOMIDI, x)
def dbamp(x): return _unop(UnaryOp.DBAMP, x)
def ampdb(x): return _unop(UnaryOp.AMPDB, x)
def octcps(x): return _unop(UnaryOp.OCTCPS, x)
def cpsoct(x): return _unop(UnaryOp.CPSOCT, x)


# ---- perceptual frequency scales (native, f64 -- not UnaryOps) ----
#
# The two scales a spectrogram's frequency axis is labeled in, named the way
# SuperCollider names a conversion (``<from><to>``, hertz spelled ``cps``), so
# they read beside ``midicps`` and ``cpsoct`` above. They are **not** members of
# the server's unary-op table -- no UGen computes a mel -- so they come from
# ``clausters_core::scale`` in **f64**, which is also what the GUI host's ruler
# and the spectrogram shader read: a headless script gets the number the window
# is drawing.

def cpsmel(x):
    """Hertz to **mel** (O'Shaughnessy): the perceptual pitch scale, ~1000 mel
    at 1 kHz by construction and increasingly compressed above it."""
    return _scale(_native.hz_to_mel, x)


def melcps(x):
    """Mel to hertz, the exact inverse of `cpsmel`. Negative input maps to 0."""
    return _scale(_native.mel_to_hz, x)


def cpsbark(x):
    """Hertz to **bark** (Traunmuller): the critical-band scale, ~8.5 bark at
    1 kHz. Zero hertz sits at ``-0.53``, which is the formula and not an error
    -- the scale is invertible across it."""
    return _scale(_native.hz_to_bark, x)


def barkcps(x):
    """Bark to hertz, the analytic inverse of `cpsbark`."""
    return _scale(_native.bark_to_hz, x)


# ---- the numeric functions that are not operators (native, f64) ----
#
# sclang's functions on a number that no UGen computes, so they are not in the
# operator table and not f32: computed in f64 as sclang computes them, once, in
# ``clausters_core::numeric``. Each takes a number or a sequence for its first
# argument; the rest are plain numbers.

Calc = _native.Calc
ScaleCalc = _native.ScaleCalc


def _calc(op, x, *args):
    if _is_seq(x):
        return [_native.calc(op, float(v), *args) for v in x]
    return _native.calc(op, float(x), *args)


def degrad(x):
    """Degrees to radians."""
    return _calc(Calc.DEGRAD, x)


def raddeg(x):
    """Radians to degrees."""
    return _calc(Calc.RADDEG, x)


def cbrt(x):
    """The real cube root."""
    return _calc(Calc.CBRT, x)


def log1p(x):
    """``log(1 + x)``, exact near zero."""
    return _calc(Calc.LOG1P, x)


def expm1(x):
    """``exp(x) - 1``, exact near zero."""
    return _calc(Calc.EXPM1, x)


def erf(x):
    """The error function."""
    return _calc(Calc.ERF, x)


def erfc(x):
    """The complementary error function, ``1 - erf(x)``, exact in its tail."""
    return _calc(Calc.ERFC, x)


def tgamma(x):
    """The gamma function."""
    return _calc(Calc.TGAMMA, x)


def lgamma(x):
    """``log |gamma(x)|``."""
    return _calc(Calc.LGAMMA, x)


def zapgremlins(x):
    """`x`, or 0 where it is a denormal, an infinity, a NaN or past 1e15 --
    what keeps a feedback path from carrying one."""
    return _calc(Calc.ZAPGREMLINS, x)


def wrap1(x):
    """`x` wrapped into ``-1..1`` by one shift."""
    return _calc(Calc.WRAP1, x)


def fold1(x):
    """`x` folded into ``-1..1`` by one reflection."""
    return _calc(Calc.FOLD1, x)


def even(x):
    """Whether the whole part of `x` is even."""
    r = _calc(Calc.EVEN, x)
    return [v != 0.0 for v in r] if _is_seq(x) else r != 0.0


def odd(x):
    """Whether the whole part of `x` is odd."""
    r = _calc(Calc.ODD, x)
    return [v != 0.0 for v in r] if _is_seq(x) else r != 0.0


def graycode(x):
    """The Gray code of the whole part of `x`: ``x ^ (x >> 1)``."""
    r = _calc(Calc.GRAYCODE, x)
    return [int(v) for v in r] if _is_seq(x) else int(r)


def nextpowerof(x, base):
    """The least power of `base` at or above `x` (a power maps to itself)."""
    return _calc(Calc.NEXTPOWEROF, x, base)


def nextpoweroftwo(x):
    """The least power of two at or above `x`."""
    return nextpowerof(x, 2.0)


def nextpowerofthree(x):
    """The least power of three at or above `x`."""
    return nextpowerof(x, 3.0)


def previouspowerof(x, base):
    """The power of `base` below the one `nextpowerof` gives."""
    return _calc(Calc.PREVIOUSPOWEROF, x, base)


def moddif(x, b=0.0, mod=1.0):
    """The distance from `x` to `b` on a circle of circumference `mod`:
    0.9 and 0.1 are 0.2 apart on the unit circle, not 0.8."""
    return _calc(Calc.MODDIF, x, b, mod)


def softround(x, resolution=1.0, margin=0.05, strength=1.0):
    """`x` pulled toward its nearest multiple of `resolution` by `strength`
    when it is **farther** than `margin` from it."""
    return _calc(Calc.SOFTROUND, x, resolution, margin, strength)


def snap(x, resolution=1.0, margin=0.05, strength=1.0):
    """`x` pulled toward its nearest multiple of `resolution` by `strength`
    when it is **within** `margin` of it -- a magnet for the grid."""
    return _calc(Calc.SNAP, x, resolution, margin, strength)


def lcurve(x, a=1.0, m=0.0, n=1.0, tau=1.0):
    """The logistic curve ``a * (m*e^-x + 1) / (n*e^-x + 1)``."""
    return _calc(Calc.LCURVE, x, a, m, n, tau)


def gausscurve(x, a=1.0, b=0.0, c=1.0):
    """The Gaussian bell ``a * exp(-(x - b)^2 / (2 c^2))``."""
    return _calc(Calc.GAUSSCURVE, x, a, b, c)


def lg3interp(x1, a, b, c, d):
    """Lagrange cubic interpolation at `x1` in ``0..1`` between `b` and `c`,
    with `a` and `d` their outer neighbours."""
    return _calc(Calc.LG3INTERP, x1, a, b, c, d)


def calcfeedback(delaytime, decaytime):
    """The feedback coefficient that decays a delay of `delaytime` by 60 dB
    in `decaytime` (a negative `decaytime` inverts it)."""
    return _calc(Calc.CALCFEEDBACK, delaytime, decaytime)


def _clip_number(clip):
    return int(Clip[clip.upper()] if isinstance(clip, str) else Clip(clip))


def bilin(x, in_center, in_lo, in_hi, out_center, out_lo, out_hi, clip="minmax"):
    """A linear map with a centre: `x` read off ``in_lo..in_center..in_hi``,
    each half mapped linearly onto its half of the output."""
    return _calc(Calc.BILIN, x, in_center, in_lo, in_hi, out_center, out_lo,
                 out_hi, _clip_number(clip))


def biexp(x, in_center, in_lo, in_hi, out_center, out_lo, out_hi, clip="minmax"):
    """`bilin` read off an exponential input."""
    return _calc(Calc.BIEXP, x, in_center, in_lo, in_hi, out_center, out_lo,
                 out_hi, _clip_number(clip))


def besselj(n, x):
    """The Bessel function of the first kind, integer order `n`, at `x`."""
    if _is_seq(x):
        return [_native.calc(Calc.BESSELJ, float(n), float(v)) for v in x]
    return _native.calc(Calc.BESSELJ, float(n), float(x))


def bessely(n, x):
    """The Bessel function of the second kind, integer order `n`, at `x`."""
    if _is_seq(x):
        return [_native.calc(Calc.BESSELY, float(n), float(v)) for v in x]
    return _native.calc(Calc.BESSELY, float(n), float(x))


def blend(a, b, frac=0.5):
    """`frac` of the way from `a` to `b`. Any of the three may be a sequence,
    and two lists blend point by point (the shorter extended cyclically)."""
    args = (a, b, frac)
    seqs = [_is_seq(v) for v in args]
    if not _py.any(seqs):
        return _native.calc(Calc.BLEND, float(a), float(b), float(frac))
    n = _py.max(len(v) for v, q in zip(args, seqs) if q)
    cols = [_extend(v, n) if q else [v] * n for v, q in zip(args, seqs)]
    return [_native.calc(Calc.BLEND, float(x), float(y), float(f))
            for x, y, f in zip(*cols)]


# ---- scales and sorted lists (native, f64) ----

def _scale_calc(op, x, values, steps):
    values = [float(v) for v in values]
    if _is_seq(x):
        return [_native.scale_calc(op, float(v), values, steps) for v in x]
    return _native.scale_calc(op, float(x), values, steps)


def degkey(degree, scale, steps=12):
    """Scale degree to key: `degree` indexes `scale` (offsets within one octave
    of `steps`) with octave carry, and a fraction is an alteration as
    SuperCollider writes it -- ``1.1`` is degree 1 raised a semitone."""
    return _scale_calc(ScaleCalc.DEGKEY, degree, scale, steps)


def keydeg(key, scale, steps=12):
    """Key to scale degree, the inverse of `degkey`: a key between two degrees
    is a fractional degree, linearly between them."""
    return _scale_calc(ScaleCalc.KEYDEG, key, scale, steps)


def nearestinscale(x, scale, steps=12):
    """The key of `scale` nearest to `x`, in `x`'s own octave of `steps`."""
    return _scale_calc(ScaleCalc.NEARESTINSCALE, x, scale, steps)


def nearestinlist(x, values):
    """The value of the sorted list `values` nearest to `x`."""
    return _scale_calc(ScaleCalc.NEARESTINLIST, x, values, 0.0)


def degree_to_midinote(degree, alter, octave, root, scale):
    """Scale degree to MIDI note number in the pitch space `octave` / `root` /
    `scale` -- the reading an `Event`'s ``degree`` key gets, and the web
    client's ``degreeToMidinote``."""
    return _native.degree_to_midinote(float(degree), float(alter), float(octave),
                                      float(root), [float(v) for v in scale])


# ---- lists (native, f64) ----
#
# sclang's functions that read or remake a whole list. The ones that draw
# (`tablerand`, `scramble`) are random values and live with the others in
# `clausters.base.rand`.

ListOp = _native.ListOp


def blendat(values, index, method="clip"):
    """`values` read at the fractional `index`, blended between the two values
    around it. `method` says what an index past either end reads: ``"clip"``
    (held at the end), ``"wrap"`` or ``"fold"``."""
    m = int(BoundOp[method.upper()]) if isinstance(method, str) else int(method)
    if _is_seq(index):
        return [_native.blend_at(values, float(i), m) for i in index]
    return _native.blend_at(values, float(index), m)


def normalize(values, lo=0.0, hi=1.0):
    """`values` mapped linearly from their own range onto ``lo..hi``."""
    return _native.list_op(ListOp.NORMALIZE, values, lo, hi)


def normalizesum(values):
    """`values` divided by their sum, so they sum to 1 -- the weights
    `wchoose` reads, made explicit."""
    return _native.list_op(ListOp.NORMALIZESUM, values)


def integrate(values):
    """The running sum of `values`."""
    return _native.list_op(ListOp.INTEGRATE, values)


def differentiate(values):
    """Each value less the one before it (the first less 0)."""
    return _native.list_op(ListOp.DIFFERENTIATE, values)


def resamp0(values, size):
    """`values` resampled to `size` values by taking the nearest."""
    return _native.list_op(ListOp.RESAMP0, values, size)


def resamp1(values, size):
    """`values` resampled to `size` values by linear interpolation."""
    return _native.list_op(ListOp.RESAMP1, values, size)


def asrandomtable(values, size=None):
    """The table `clausters.tablerand` draws from, with `values` as the
    probability shape (higher is likelier), `size` entries long (the shape's
    own length unless given)."""
    return _native.list_op(ListOp.ASRANDOMTABLE, values,
                           len(values) if size is None else size)


def indexinbetween(values, x):
    """The fractional index of `x` in the sorted list `values`."""
    return _scale_calc(ScaleCalc.INDEXINBETWEEN, x, values, 0.0)


def indexofgreaterthan(values, x):
    """The first index of the sorted list `values` greater than `x`, or
    ``None`` where none is."""
    i = _native.scale_calc(ScaleCalc.INDEXOFGREATERTHAN, float(x), [float(v) for v in values])
    return None if i < 0 else int(i)


# ---- range maps (native, f32 -- SuperCollider's warp family) ----
#
# Reading a value out of one range and writing it into another, which is what a
# control's position, a spec and an envelope's curve all are. The formulas live
# once in ``clausters_core::warp``, so a value mapped here, the same map on the
# audio thread and the curve the GUI host draws are one curve.
#
# ``clip`` says what an out-of-range input is trimmed to before it is mapped:
# ``"minmax"`` (both ends, SuperCollider's default), ``"min"``, ``"max"``, or
# ``"none"`` to extrapolate.

def _map(op, x, in_lo, in_hi, out_lo, out_hi, curve=0.0, clip="minmax"):
    mode = Clip[clip.upper()] if isinstance(clip, str) else Clip(clip)
    if _is_seq(x):
        x = [float(v) for v in x]
    return _native.map_range(op, mode, x, float(in_lo), float(in_hi),
                             float(out_lo), float(out_hi), float(curve))


def linlin(x, in_lo, in_hi, out_lo, out_hi, clip="minmax"):
    """`x` off a linear range onto a linear one."""
    return _map(MapOp.LINLIN, x, in_lo, in_hi, out_lo, out_hi, clip=clip)


def linexp(x, in_lo, in_hi, out_lo, out_hi, clip="minmax"):
    """`x` off a linear range onto an exponential one -- a fader position to a
    frequency. The output ends must not straddle zero; one *at* zero is nudged
    to the smallest same-signed value rather than giving a NaN."""
    return _map(MapOp.LINEXP, x, in_lo, in_hi, out_lo, out_hi, clip=clip)


def explin(x, in_lo, in_hi, out_lo, out_hi, clip="minmax"):
    """`x` off an exponential range onto a linear one -- a frequency to a fader
    position."""
    return _map(MapOp.EXPLIN, x, in_lo, in_hi, out_lo, out_hi, clip=clip)


def expexp(x, in_lo, in_hi, out_lo, out_hi, clip="minmax"):
    """`x` off an exponential range onto another."""
    return _map(MapOp.EXPEXP, x, in_lo, in_hi, out_lo, out_hi, clip=clip)


def lincurve(x, in_lo, in_hi, out_lo, out_hi, curve=-4.0, clip="minmax"):
    """`x` off a linear range onto one **bent** by ``curve``: 0 is linear,
    negative builds fast then slow -- most of the output spent on the first half
    of the input -- and positive the reverse, which is the fine-at-the-bottom
    feel a frequency or an amplitude control wants. Unlike `linexp` the bend
    spans zero freely."""
    return _map(MapOp.LINCURVE, x, in_lo, in_hi, out_lo, out_hi, curve, clip)


def curvelin(x, in_lo, in_hi, out_lo, out_hi, curve=-4.0, clip="minmax"):
    """The inverse of `lincurve`: off a bent range onto a linear one."""
    return _map(MapOp.CURVELIN, x, in_lo, in_hi, out_lo, out_hi, curve, clip)


def range(x, lo=0.0, hi=1.0):
    """A **bipolar** value (``-1``..``1``) onto ``lo``..``hi``, linearly.

    Nothing is trimmed: a UGen knows its own signal range and a bare value does
    not, so an input past ``-1``..``1`` overshoots the output by the same
    proportion instead of being clipped to an assumption."""
    return _map(MapOp.RANGE, x, -1.0, 1.0, lo, hi, clip="none")


def exprange(x, lo=0.01, hi=1.0):
    """A **bipolar** value onto ``lo``..``hi`` exponentially. Untrimmed, for the
    reason `range` gives."""
    return _map(MapOp.EXPRANGE, x, -1.0, 1.0, lo, hi, clip="none")


# Wire selector -> function, so AbstractObject value subclasses can dispatch by
# the operator names the graph layer puts on the wire. The two spellings differ
# where SuperCollider's name and the protocol's do (`asint` -> `asinteger`):
# the key is the def format's, the value is this client's.
UNARY = {
    "neg": neg, "abs": abs, "sin": sin, "cos": cos, "tan": tan, "asin": asin,
    "acos": acos, "atan": atan, "exp": exp, "exp10": exp10, "log": log, "log10": log10,
    "log2": log2, "sqrt": sqrt, "floor": floor, "ceil": ceil, "rint": rint,
    "asint": asinteger, "asfloat": asfloat, "squared": squared, "cubed": cubed,
    "recip": reciprocal, "frac": frac, "sign": sign, "sinh": sinh, "cosh": cosh,
    "tanh": tanh, "distort": distort, "softclip": softclip, "midicps": midicps,
    "cpsmidi": cpsmidi, "midiratio": midiratio, "ratiomidi": ratiomidi,
    "dbamp": dbamp, "ampdb": ampdb, "octcps": octcps, "cpsoct": cpsoct,
    "bitnot": bitnot, "rectwindow": rectwindow, "hanwindow": hanwindow,
    "welwindow": welwindow, "triwindow": triwindow, "ramp": ramp,
    "scurve": scurve,
}
BINARY = {
    "add": add, "sub": sub, "mul": mul, "div": div, "mod": mod, "pow": pow,
    "min": min, "max": max, "atan2": atan2, "gt": gt, "lt": lt, "ge": ge,
    "le": le, "eq": eq, "ne": ne, "bitand": bitand, "bitor": bitor,
    "bitxor": bitxor, "lshift": leftshift, "rshift": rightshift, "hypot": hypot,
    "ring1": ring1, "ring2": ring2, "ring3": ring3, "ring4": ring4,
    "sumsqr": sumsqr, "difsqr": difsqr, "sqrsum": sqrsum, "sqrdif": sqrdif,
    "absdif": absdif, "thresh": thresh, "clip2": clip2, "excess": excess,
    "round": round, "trunc": trunc, "fold2": fold2, "wrap2": wrap2,
    "gcd": gcd, "lcm": lcm, "hypotapx": hypotapx, "idiv": idiv,
    "roundup": roundup, "urshift": urshift, "amclip": amclip,
    "scaleneg": scaleneg, "firstarg": firstarg,
}
BOUND = {"clip": clip, "wrap": wrap, "fold": fold}
