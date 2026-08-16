"""UGen graph as composable, lowercase callables (port of the UGen side of
``sc3/synth``, adapted to Clausters' server format).

The UGen-graph counterpart of `clausters.defs.signals`: each function here
is a small **lowercase** callable that returns a `Ugen` node (one
output); composing nodes with Python operators or these functions builds the
graph a `SynthDef` serializes into the JSON
``SynthDefSpec`` the server's ``/def_send synth`` consumes (``{"controls": […],
"ugens": […]}`` — see the server's ``synthdef`` module).

**Instance-based, no global build context.** Unlike sclang — where ``SynthDef``
build relies on a thread-global "current graph" that every ``UGen.new`` mutates
(``UGen.buildSynthDef``) — the graph here *is* the tree of composed objects: a
``Ugen``'s inputs hold its operands directly, and the `SynthDef` walks
that tree to emit the spec. Nothing is global, so several defs can be built
concurrently.

**The server's UGen set is deliberately focused**: oscillators/sources
(``sine``, ``impulse``, ``white_noise``, `rand`), the table oscillators and
waveshaper (``osc``/``oscn``/``vosc``, ``shaper``), bus and buffer I/O
(``in_``/``in_ctl``, ``out``/``replace_out``, ``play_buf``/``buf_rd``, the
``buf_*`` info queries), streaming disk I/O (``disk_in``/``disk_out``),
feedback (``local_in``/``local_out``), the ``env_gen`` envelope, the ``lag``/
``var_lag`` smoothers, the demand pair (``dseq``/``demand``), and the fused
``madd``/``sum3``/``sum4``. **Maths works**: ``+ - * /`` map to the
``Add``/``Sub``/``Mul``/``Div`` kinds and every other operator or method
(``%``, ``min``/``max``, comparisons, ``.sin()``, ``.midicps()``,
``.distort()`` …) composes a generic ``BinaryOpUGen``/``UnaryOpUGen`` carrying
the operator name — the same op the value side computes, so the two agree
bit-for-bit. Reach for a Faust def (`clausters.defs.signals`) only for genuinely
custom per-sample DSP (recursion, tables, sample-accurate feedback).

**Multichannel is an explicit container**, not implicit expansion: `dup` fans
a signal out into a `ChannelList` (by reference for a node, by evaluation for
a callable), operators broadcast/zip over it (wrapping the shorter side
modulo, the value side's rule), ``out(bus, chans)`` lays the channels on
consecutive buses, and `mix` folds a list back to one channel through the
fused sums. sclang-style per-argument expansion (``sine([440, 443])``) is
deliberately **not** implemented — a channel list reaching a single-channel
input is a `TypeError` at serialization.

Each UGen output carries a **rate** (``ir``/``kr``/``ar``/``dr``); it defaults
per kind and can be set with `Ugen.at_rate`. Controls carry a **type** and an
optional **lag** — see `control`/`Control`.

Envelopes are the `Env` breakpoint builder plus the `env_gen` callable, which
serialize to the ``EnvGen`` UGen's flat input list.

Reserved controls ``in`` and ``out`` (the input/output buses, set with
``/synth_new … "in" b "out" b``) are added by the server, not declared here.

**Where things live.** The callables are grouped by family, one module each:
`graph` (the node, control and channel-list types, plus the fused arithmetic),
`osc`, `filter` (filters, delays, smoothers), `pan`, `io` (buses, replies, disk,
feedback), `buf`, `spectral`, `trig`, `demand` and `env`. Every name is
re-exported here, so `from clausters.defs.ugens import sine` — and the `defs`
package's own re-export — is what it always was: the split is navigational.

`ugen_input_names` stays in this file rather than in a family module because it
reads the **package's** namespace: it maps each server kind to the parameter
names of the callable that builds it, and only here are all the builders in one
`globals()`.
"""

# The private names other modules in the client import from here: the operator
# tables `pv_expr` reads and the curve resolver `gui.guidef` reaches for. They
# were importable when this was one module, so they stay importable.
from .graph import _BINOP_OPS, _UNOP_OPS
from .env import _resolve_curve

from .graph import (
    ChannelList,
    Control,
    Ugen,
    chans,
    control,
    dup,
    madd,
    mix,
    sum3,
    sum4,
)
from .osc import (
    brown_noise,
    clip_noise,
    crackle,
    dust,
    dust2,
    gray_noise,
    impulse,
    lf_clip_noise,
    lf_noise0,
    lf_noise1,
    lf_noise2,
    lf_pulse,
    lf_saw,
    lf_tri,
    phasor,
    record_buf,
    transport_pos,
    pink_noise,
    pulse,
    saw,
    sine,
    var_saw,
    white_noise,
)
from .filter import (
    allpass_c,
    allpass_l,
    allpass_n,
    bpf,
    brf,
    comb_c,
    comb_l,
    comb_n,
    delay_c,
    delay_l,
    delay_n,
    hpf,
    integrator,
    lag,
    leak_dc,
    lpf,
    one_pole,
    one_zero,
    resonz,
    rhpf,
    rlpf,
    svf,
    svf_morph,
    var_lag,
)
from .pan import (
    balance2,
    lin_pan2,
    lin_xfade2,
    mid_side,
    pan2,
    pan_az,
    rotate2,
    select,
    select_x,
    splay,
    stereo_width,
    xfade2,
)
from .io import (
    disk_in,
    disk_out,
    in_,
    in_ctl,
    local_in,
    local_out,
    out,
    out_ctl,
    poll,
    replace_out,
    send_reply,
    send_trig,
)
from .buf import (
    buf_channels,
    buf_dur,
    buf_frames,
    buf_rate_scale,
    buf_rd,
    buf_wr,
    buf_sample_rate,
    osc,
    oscn,
    play_buf,
    rand,
    sample_rate,
    shaper,
    vosc,
)
from .spectral import (
    conv,
    fft,
    ifft,
    partconv_frames,
    pv_add,
    pv_bin_shift,
    pv_brick_wall,
    pv_copy_phase,
    pv_kernel,
    pv_mag_above,
    pv_mag_below,
    pv_mag_clip,
    pv_mag_freeze,
    pv_mag_mul,
    pv_mag_shift,
    pv_mag_smear,
    pv_max,
    pv_min,
    pv_mul,
)
from .trig import (
    changed,
    decay,
    decay2,
    gate,
    latch,
    pulse_count,
    pulse_divider,
    schmidt,
    set_reset_ff,
    stepper,
    sweep,
    t_delay,
    timer,
    toggle_ff,
    trig,
    trig1,
)
from .demand import (
    dbrown,
    dbufrd,
    demand,
    dgeom,
    dibrown,
    diwhite,
    drand,
    dseq,
    dseries,
    dshuf,
    dstutter,
    dswitch1,
    duty,
    dwhite,
    dxrand,
    tduty,
)
from .env import (
    DoneAction,
    Env,
    detect_silence,
    done,
    env_gen,
    env_to_points,
    free_self,
    free_self_when_done,
    line,
    pause_self,
    points_to_env,
    x_line,
)

# ---- introspecting a UGen kind's input names (the client's own signature) ----
#
# The level-2 Def-view labels a UGen box's inlets from the client's own
# vocabulary: the parameter names of the callable that builds the kind. That
# callable *is* the client's mirror of the server registry (the /ugen_query
# contrast test keeps the two in line, see `tests/test_session.py`), so reusing
# it here means the patcher and the builder never disagree on an input's name.

#: Kinds whose builder's positional parameters do **not** line up with the wire
#: input order (variadic runs, static fields sitting between inputs) — the
#: divergences the /ugen_query contrast test declares. For these the names would
#: mislabel the inlets, so the Def-view falls back to positional labels.
_INPUT_NAMES_MISALIGNED = frozenset(
    {"EnvGen", "SendReply", "Dseq", "Poll", "DiskIn", "DiskOut", "PV_Kernel"}
)

#: Lazily built {kind: [param name, ...]} — see `ugen_input_names`.
_INPUT_NAMES: "dict[str, list[str]] | None" = None


def _build_input_names() -> dict:
    """Map each server UGen kind to its builder callable's positional parameter
    names, read from the ``Ugen("Kind", ...)`` literal in this module's source
    (the function name does not equal the kind: ``in_`` builds ``In``,
    ``oscn`` builds ``OscN``)."""
    import ast
    import inspect

    names: dict[str, list[str]] = {}
    for fname, fn in list(globals().items()):
        if fname.startswith("_") or not inspect.isfunction(fn):
            continue
        try:
            tree = ast.parse(inspect.getsource(fn).lstrip())
        except (OSError, SyntaxError):
            continue
        kind = None
        for node in ast.walk(tree):
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                    and node.func.id == "Ugen" and node.args
                    and isinstance(node.args[0], ast.Constant)):
                kind = node.args[0].value
                break
        if kind is None or kind in names:
            continue
        names[kind] = [
            p.name for p in inspect.signature(fn).parameters.values()
            if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)
        ]
    return names


def ugen_input_names(kind: str) -> "list[str] | None":
    """The positional input names of the callable that builds UGen ``kind``, or
    ``None`` when no single callable maps to it cleanly — the generic op UGens
    (``BinaryOpUGen``/``UnaryOpUGen``, built inline) and the kinds whose builder
    parameters do not line up with the wire order (`_INPUT_NAMES_MISALIGNED`).
    A ``None`` result means the caller labels the inlets positionally."""
    global _INPUT_NAMES
    if kind in _INPUT_NAMES_MISALIGNED:
        return None
    if _INPUT_NAMES is None:
        _INPUT_NAMES = _build_input_names()
    return _INPUT_NAMES.get(kind)


__all__ = [
    # graph
    "ChannelList",
    "Control",
    "Ugen",
    "chans",
    "control",
    "dup",
    "madd",
    "mix",
    "sum3",
    "sum4",
    # osc
    "brown_noise",
    "clip_noise",
    "crackle",
    "dust",
    "dust2",
    "gray_noise",
    "impulse",
    "lf_clip_noise",
    "lf_noise0",
    "lf_noise1",
    "lf_noise2",
    "lf_pulse",
    "lf_saw",
    "lf_tri",
    "phasor",
    "record_buf",
    "transport_pos",
    "pink_noise",
    "pulse",
    "saw",
    "sine",
    "var_saw",
    "white_noise",
    # filter
    "allpass_c",
    "allpass_l",
    "allpass_n",
    "bpf",
    "brf",
    "comb_c",
    "comb_l",
    "comb_n",
    "delay_c",
    "delay_l",
    "delay_n",
    "hpf",
    "integrator",
    "lag",
    "leak_dc",
    "lpf",
    "one_pole",
    "one_zero",
    "resonz",
    "rhpf",
    "rlpf",
    "svf",
    "svf_morph",
    "var_lag",
    # pan
    "balance2",
    "lin_pan2",
    "lin_xfade2",
    "mid_side",
    "pan2",
    "pan_az",
    "rotate2",
    "select",
    "select_x",
    "splay",
    "stereo_width",
    "xfade2",
    # io
    "disk_in",
    "disk_out",
    "in_",
    "in_ctl",
    "local_in",
    "local_out",
    "out",
    "out_ctl",
    "poll",
    "replace_out",
    "send_reply",
    "send_trig",
    # buf
    "buf_channels",
    "buf_dur",
    "buf_frames",
    "buf_rate_scale",
    "buf_rd",
    "buf_wr",
    "buf_sample_rate",
    "osc",
    "oscn",
    "play_buf",
    "rand",
    "sample_rate",
    "shaper",
    "vosc",
    # spectral
    "conv",
    "fft",
    "ifft",
    "partconv_frames",
    "pv_add",
    "pv_bin_shift",
    "pv_brick_wall",
    "pv_copy_phase",
    "pv_kernel",
    "pv_mag_above",
    "pv_mag_below",
    "pv_mag_clip",
    "pv_mag_freeze",
    "pv_mag_mul",
    "pv_mag_shift",
    "pv_mag_smear",
    "pv_max",
    "pv_min",
    "pv_mul",
    # trig
    "changed",
    "decay",
    "decay2",
    "gate",
    "latch",
    "pulse_count",
    "pulse_divider",
    "schmidt",
    "set_reset_ff",
    "stepper",
    "sweep",
    "t_delay",
    "timer",
    "toggle_ff",
    "trig",
    "trig1",
    # demand
    "dbrown",
    "dbufrd",
    "demand",
    "dgeom",
    "dibrown",
    "diwhite",
    "drand",
    "dseq",
    "dseries",
    "dshuf",
    "dstutter",
    "dswitch1",
    "duty",
    "dwhite",
    "dxrand",
    "tduty",
    # env
    "DoneAction",
    "Env",
    "detect_silence",
    "done",
    "env_gen",
    "env_to_points",
    "free_self",
    "free_self_when_done",
    "line",
    "pause_self",
    "points_to_env",
    "x_line",
    "ugen_input_names",
]
