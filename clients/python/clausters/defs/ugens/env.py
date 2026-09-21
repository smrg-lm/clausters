"""Envelopes: the breakpoint builder, the generator, and what `done` frees.

`Env` is the shape, `env_gen` plays it, and a `DoneAction` says what becomes of
the node when it ends -- the one place in the package where a UGen's completion
reaches the node tree.
"""

from .graph import Ugen

# ---- envelopes (EnvGen) ----


class DoneAction:
    """The action `env_gen` takes when its envelope finishes -- scsynth's full
    done-action set (0-15). Pass one as ``done_action``. The relative actions
    (3-13, 15) act on the synth's neighbours in its group; a paused node is
    resumed with `Server.run` (``/node_run``)."""

    #: Do nothing; the envelope just holds its final level.
    NONE = 0
    #: Pause the synth (stops processing; it stays in the tree). Resume with
    #: `Server.run`.
    PAUSE_SELF = 1
    #: Free the synth -- the usual choice for a one-shot or a released note.
    FREE_SELF = 2
    #: Free the synth and the preceding node.
    FREE_SELF_AND_PREV = 3
    #: Free the synth and the following node.
    FREE_SELF_AND_NEXT = 4
    #: Free the synth; if the preceding node is a group, free all its children.
    FREE_SELF_AND_FREE_ALL_IN_PREV = 5
    #: Free the synth; if the following node is a group, free all its children.
    FREE_SELF_AND_FREE_ALL_IN_NEXT = 6
    #: Free the synth and every preceding node in its group.
    FREE_SELF_TO_HEAD = 7
    #: Free the synth and every following node in its group.
    FREE_SELF_TO_TAIL = 8
    #: Free the synth and pause the preceding node.
    FREE_SELF_PAUSE_PREV = 9
    #: Free the synth and pause the following node.
    FREE_SELF_PAUSE_NEXT = 10
    #: Free the synth; if the preceding node is a group, deep-free it.
    FREE_SELF_AND_DEEP_FREE_PREV = 11
    #: Free the synth; if the following node is a group, deep-free it.
    FREE_SELF_AND_DEEP_FREE_NEXT = 12
    #: Free the synth and every other node in its group.
    FREE_ALL_IN_GROUP = 13
    #: Free the synth's whole enclosing group.
    FREE_GROUP = 14
    #: Free the synth and resume (unpause) the following node.
    FREE_SELF_RESUME_NEXT = 15


#: Envelope shape name -> the server's shape number. A numeric curve value maps
#: to the custom-curvature shape (5) instead.
_SHAPE_NUMBERS = {
    "step": 0,
    "lin": 1,
    "linear": 1,
    "exp": 2,
    "exponential": 2,
    "sin": 3,
    "sine": 3,
    "wel": 4,
    "welch": 4,
    "sqr": 6,
    "squared": 6,
    "cub": 7,
    "cubed": 7,
    "hold": 8,
}


def _resolve_curve(spec):
    """A shape name (``'lin'``, ``'exp'``, ``'sin'`` ...) or a numeric curvature
    -> ``(shape_number, curve_value)``. A number selects the custom-curvature
    shape, where 0 is linear, positive starts slow, negative starts fast."""
    if isinstance(spec, str):
        try:
            return (_SHAPE_NUMBERS[spec], 0.0)
        except KeyError:
            raise ValueError(
                f"unknown envelope shape {spec!r}; use one of "
                f"{sorted(set(_SHAPE_NUMBERS))} or a numeric curvature"
            ) from None
    return (5, float(spec))


class Env:
    """A breakpoint envelope: `levels` (one more than `times`), the segment
    `times` in seconds, and a `curve` per segment (a shape name, a numeric
    curvature, or a list of either, one per segment).

    `release_node` is the index into `levels` where the envelope sustains while
    the gate is held (``None`` = no sustain, plays straight through). Feed it to
    `env_gen`. Modelled on SuperCollider's ``Env``; the shapes match the
    server's `EnvGen`."""

    def __init__(self, levels, times, curve="lin", release_node=None, loop_node=None):
        self.levels = [float(x) for x in levels]
        self.times = [float(x) for x in times]
        if len(self.levels) != len(self.times) + 1:
            raise ValueError(
                f"levels ({len(self.levels)}) must be one longer than "
                f"times ({len(self.times)})"
            )
        if isinstance(curve, (list, tuple)):
            if len(curve) != len(self.times):
                raise ValueError(
                    f"curve list ({len(curve)}) must match the number of "
                    f"segments ({len(self.times)})"
                )
            self.curves = list(curve)
        else:
            self.curves = [curve] * len(self.times)
        self.release_node = release_node
        self.loop_node = loop_node

    @classmethod
    def perc(cls, attack=0.01, release=1.0, level=1.0, curve=-4.0):
        """A fixed-duration percussive hit: 0 -> `level` -> 0. No sustain, so a
        rising gate triggers the whole thing."""
        return cls([0.0, level, 0.0], [attack, release], curve)

    @classmethod
    def adsr(
        cls,
        attack=0.01,
        decay=0.3,
        sustain=0.5,
        release=1.0,
        peak=1.0,
        curve=-4.0,
    ):
        """The classic attack/decay/sustain/release. Sustains at ``peak *
        sustain`` (the release node) until the gate falls."""
        return cls(
            [0.0, peak, peak * sustain, 0.0],
            [attack, decay, release],
            curve,
            release_node=2,
        )

    @classmethod
    def asr(cls, attack=0.01, sustain=1.0, release=1.0, curve=-4.0):
        """Attack to `sustain`, hold there until release, then fall to 0."""
        return cls([0.0, sustain, 0.0], [attack, release], curve, release_node=1)

    @classmethod
    def step(cls, levels, times, release_node=None, loop_node=None):
        """A step sequence: **each value held for its duration** -- `levels`
        and `times` have the *same* length, unlike the raw constructor
        (``Env.step([0, 1], [0.5, 0.5])`` holds 0 for 0.5, then 1 for 0.5).

        This is the conceptual interface of a value-with-duration sequence;
        like SuperCollider's ``Env.step``, it is expressed over the raw
        initial-level + (target, duration) form by prepending the first level
        with the ``"step"`` shape (which jumps to each segment's target at its
        start)."""
        levels = list(levels)
        times = list(times)
        if len(levels) != len(times):
            raise ValueError(
                f"Env.step: levels ({len(levels)}) and times ({len(times)}) "
                "must have the same length"
            )
        if not levels:
            raise ValueError("Env.step needs at least one level")
        return cls([levels[0]] + levels, times, "step",
                   release_node=release_node, loop_node=loop_node)

    def to_points(self, *, time_at: float = 0.0) -> list:
        """This envelope in the **other basis**: the flat ``[t, v, shape,
        curve, ...]`` break points a `Bpf` is written with, absolute from
        ``time_at``. The conversion is `env_to_points`."""
        return env_to_points(self, time_at=time_at)

    def set_points(self, points, *, time_at: float = 0.0) -> "Env":
        """Take this envelope's shape from a flat break-point list, in place.

        The other half of `to_points`, and what makes an `Env` editable by
        anything that draws break points. ``release_node`` and ``loop_node``
        survive when they still index a level and are dropped when they do not
        -- a curve redrawn with fewer points has no say about where the old
        sustain was."""
        fresh = points_to_env(points, time_at=time_at)
        self.levels, self.times, self.curves = fresh.levels, fresh.times, fresh.curves
        for attr in ("release_node", "loop_node"):
            node = getattr(self, attr)
            if node is not None and int(node) >= len(self.levels):
                setattr(self, attr, None)
        return self

    def to_inputs(self):
        """The envelope as the flat number list `env_gen` appends after its
        fixed inputs: ``initLevel, numSegments, releaseNode, loopNode`` then
        ``target, duration, shape, curve`` per segment."""
        n = len(self.times)
        rel = -1.0 if self.release_node is None else float(self.release_node)
        loop = -1.0 if self.loop_node is None else float(self.loop_node)
        out = [self.levels[0], float(n), rel, loop]
        for i in range(n):
            shape, cval = _resolve_curve(self.curves[i])
            out += [self.levels[i + 1], self.times[i], float(shape), cval]
        return out


def env_gen(
    env: Env,
    gate=1.0,
    level_scale=1.0,
    level_bias=0.0,
    time_scale=1.0,
    done_action=DoneAction.NONE,
) -> Ugen:
    """Plays an `Env`. A rising `gate` (re)triggers from the start; while the
    gate is held the envelope sustains at the env's release node; when the gate
    falls it plays the release segments. `level_scale`/`level_bias` affine the
    output, `time_scale` stretches every segment. `done_action` is taken when
    the envelope finishes (see `DoneAction`)."""
    fixed = [gate, level_scale, level_bias, time_scale, float(done_action)]
    return Ugen("EnvGen", fixed + env.to_inputs())


def detect_silence(signal, amp=0.0001, time=0.1, done_action=DoneAction.NONE) -> Ugen:
    """1 once ``signal`` has stayed within ``+/-amp`` for ``time`` seconds, with
    the ``done_action`` taken then.

    The counter restarts on the first sample that exceeds ``amp``, so what it
    measures is *uninterrupted* silence. It raises a done flag, so `done` and
    `free_self_when_done` can watch it: the usual way to let a voice leave when
    it has nothing left to say."""
    return Ugen("DetectSilence", [signal, amp, time, float(done_action)])


def line(start=0.0, end=1.0, dur=1.0, done_action=DoneAction.NONE) -> Ugen:
    """A single ramp from ``start`` to ``end`` over ``dur`` seconds, then held.

    It is an `env_gen` with one linear segment, so it takes the same
    `DoneAction` set -- ``line(1, 0, 2, DoneAction.FREE_SELF)`` is a two-second
    fade that frees its synth. Cheap at ``rate="kr"``, which is where a sweep
    usually belongs."""
    return Ugen("Line", [start, end, dur, float(done_action)])


def x_line(start=0.01, end=1.0, dur=1.0, done_action=DoneAction.NONE) -> Ugen:
    """`line` in equal *ratios* rather than equal steps -- the shape that reads
    as straight when it drives a frequency or a gain.

    ``start`` and ``end`` must be non-zero and share a sign; a zero is nudged to
    a tiny value of the same sign rather than producing a ``NaN``."""
    return Ugen("XLine", [start, end, dur, float(done_action)])


def free_self(signal) -> Ugen:
    """Frees the enclosing synth while ``signal`` is greater than zero, passing
    it through unchanged. The trigger-driven counterpart of a `DoneAction` --
    use it when what ends the note is not the envelope."""
    return Ugen("FreeSelf", [signal])


def pause_self(signal) -> Ugen:
    """Pauses the enclosing synth while ``signal`` is greater than zero, passing
    it through. Resume with `Server.run`; it re-pauses only if the signal is
    still up, so this is a gate rather than a one-way door."""
    return Ugen("PauseSelf", [signal])


def done(source) -> Ugen:
    """1 once ``source`` has finished, 0 before -- a trigger the rest of the
    graph can read.

    ``source`` must be a ugen that *can* finish (`env_gen`, `line`, `x_line`);
    the server rejects the def by name otherwise. What it reads is the done
    flag, not the value on the wire: an envelope that has played out sits at
    its final level, which tells you nothing."""
    return Ugen("Done", [source])


def free_self_when_done(source) -> Ugen:
    """Passes ``source`` through and frees the synth once it has finished. The
    idiom for an envelope whose own ``done_action`` is `DoneAction.NONE`
    because something else in the graph still needs it."""
    return Ugen("FreeSelfWhenDone", [source])


# ---- break-point <-> Env mapping (the Env <-> Bpf seam, and the bpf widget) ----


def env_to_points(env, *, time_at: float = 0.0) -> list:
    """An `Env` (levels / segment times / curves) as the flat ``bpf`` breakpoint
    list ``[t, v, shape, curve, ...]``, with absolute times starting at
    ``time_at``. The last point carries a linear placeholder (no segment leaves
    it). Feed the result to the ``bpf`` widget or to a live ``points`` set."""
    out: list = []
    t = float(time_at)
    for i, level in enumerate(env.levels):
        if i < len(env.times):
            shape, curve = _resolve_curve(env.curves[i])
        else:
            shape, curve = 1, 0.0
        out += [t, float(level), int(shape), float(curve)]
        if i < len(env.times):
            t += float(env.times[i])
    return out


def points_to_env(points, *, time_at: float = 0.0, **env_kwargs):
    """A ``bpf`` breakpoint list -- the flat ``t v shape curve ...`` quads a
    ``"points"`` event carries -- as an `Env`: absolute times become segment
    durations and each segment keeps its shape (the numeric curvature for the
    custom shape, the shape name otherwise).

    A first breakpoint later than ``time_at`` (default ``0.0``) is a drawn
    initial delay, encoded as a leading ``hold`` segment (the first level held
    for that duration) so what was drawn and what plays stay identical. Extra
    keywords (``release_node``, ``loop_node``) pass through to `Env`."""
    quads = [points[i:i + 4] for i in range(0, len(points) - len(points) % 4, 4)]
    if len(quads) < 2:
        raise ValueError("an envelope needs at least two breakpoints")
    # First name wins for aliased numbers ("step"/"lin"/"exp"... are listed
    # before their long forms).
    names: dict = {}
    for name, num in _SHAPE_NUMBERS.items():
        names.setdefault(num, name)
    levels = [float(q[1]) for q in quads]
    times = [float(b[0]) - float(a[0]) for a, b in zip(quads, quads[1:])]
    curve = [float(q[3]) if int(q[2]) == 5 else names.get(int(q[2]), "lin")
             for q in quads[:-1]]
    delay = float(quads[0][0]) - float(time_at)
    if delay > 1e-9:
        levels.insert(0, levels[0])
        times.insert(0, delay)
        curve.insert(0, "hold")
    return Env(levels, times, curve, **env_kwargs)


def env_gen_args(env) -> list:
    """An envelope as the flat ``/buffer_gen "env"`` argument list: ``level0``,
    then a ``(level, time, shape, curve)`` quad per segment.

    The server evaluates it with the same shape maths `env_gen` plays
    (``clausters_core::envshape``), so a buffer filled this way and an `EnvGen`
    reading the same envelope agree. Segment times are **relative** here -- only
    their proportions matter, since what maps the buffer onto real time is
    whatever reads it. Takes an `Env` or a `Bpf`.
    """
    if not isinstance(env, Env):
        env = env.to_env()
    args = [float(env.levels[0])]
    for k in range(len(env.times)):
        shape, curve = _resolve_curve(env.curves[k])
        args += [float(env.levels[k + 1]), float(env.times[k]), int(shape), float(curve)]
    return args


def quads(points, curve=None) -> list:
    """A break-point list as ``(at, value, shape, curve)`` tuples.

    Every spelling of a break point is read here, and a point is written the way
    an `Env` segment is -- **with the curve's name**, not with the server's shape
    number:

    - ``(at, value)`` -- linear, or whatever ``curve`` says;
    - ``(at, value, "exp")`` -- a shape name or a numeric curvature, the same
      values `Env`'s ``curve`` takes, resolved by `_resolve_curve`;
    - ``(at, value, shape, curve)`` -- the resolved pair, which is what the
      ``bpf`` widget sends and what a ``"points"`` event carries;
    - a flat ``[t, v, shape, curve, ...]`` list of numbers -- the same resolved
      form, dropping a trailing partial quad rather than guessing at it.

    ``curve`` is the shape for the points that do not name one: a single name or
    curvature for all of them, or a list with one per **segment** (one fewer
    than the points, as `Env` counts them). A point that names its own shape
    keeps it.
    """
    if points and not isinstance(points[0], (list, tuple)):
        flat = [float(x) for x in points]
        points = [flat[i:i + 4]
                  for i in range(0, len(flat) - len(flat) % 4, 4)]
    points = [tuple(point) for point in points]
    if isinstance(curve, (list, tuple)):
        segments = max(len(points) - 1, 0)
        if len(curve) != segments:
            raise ValueError(
                f"curve list ({len(curve)}) must match the number of "
                f"segments ({segments})"
            )
        curves = list(curve) + ["lin"]
    else:
        # The last point's shape is a placeholder -- no segment leaves it -- so
        # a blanket `curve` stops one short, which is what `env_to_points`
        # writes and what keeps a round trip identical.
        curves = [("lin" if curve is None else curve)] * max(len(points) - 1, 0)
        curves.append("lin")
    out = []
    for point, default in zip(points, curves):
        if len(point) == 2:
            shape, value = _resolve_curve(default)
        elif len(point) == 3:
            shape, value = _resolve_curve(point[2])
        elif len(point) == 4:
            shape, value = int(point[2]), float(point[3])
        else:
            raise ValueError(
                f"a break point is (at, value), (at, value, curve) or "
                f"(at, value, shape, curve); got {len(point)} numbers"
            )
        out.append((float(point[0]), float(point[1]), shape, value))
    return out


class Bpf:
    """A break-point curve: the same envelope an `Env` is, in **absolute
    coordinates**.

    One datum, two bases. An `Env` says ``levels``, segment ``times`` and a
    ``curve`` per segment, which is what `env_gen` and ``/buffer_gen "env"``
    read; a `Bpf` says ``(at, value, shape, curve)`` per point, with ``at`` an
    absolute second and the shape belonging to the segment that *leaves* that
    point, which is what a drawn curve is and what the ``bpf`` widget sends. The
    conversion both ways is `env_to_points` / `points_to_env`, and the two are
    interchangeable wherever a curve is asked for: `env_gen` plays either,
    `clausters.plot` draws either, and `clausters.gui.edit` opens either.

    It is a **specification and nothing else** -- no buffer, no bus, no
    server. Rendering a control curve to something a node can read is the
    multitrack's job and it is done in the shared crate
    (``clausters_core::mixer``), once, for every client.

    Usage::

        curve = Bpf([(0.0, 200.0), (2.0, 4000.0)], curve="exp")
        play(Synth("filter", cutoff=env_gen(curve)))
    """

    #: The unit this object's length is in -- **seconds**, like the `Env` it is
    #: another spelling of.
    duration_unit = "seconds"

    def __init__(self, points, *, curve=None, release_node=None, loop_node=None):
        self.points = quads(points, curve)
        if len(self.points) < 2:
            raise ValueError("a curve needs at least two break points")
        #: The index of the point the curve sustains at while a gate is held,
        #: and the one a loop returns to -- an `Env`'s two, in this basis, since
        #: a point here is a level there.
        self.release_node = release_node
        self.loop_node = loop_node

    @classmethod
    def from_env(cls, env, *, time_at: float = 0.0) -> "Bpf":
        """The same curve, read out of an `Env`."""
        return cls(env_to_points(env, time_at=time_at),
                   release_node=env.release_node, loop_node=env.loop_node)

    def to_env(self, *, time_at: float = 0.0) -> Env:
        """The same curve as an `Env`, which is what plays it.

        A first point later than ``time_at`` is a drawn initial delay and
        becomes a leading ``hold`` segment (`points_to_env` says why), so the
        sustain and loop indices move with it."""
        env = points_to_env(self.to_points(), time_at=time_at)
        shift = len(env.levels) - len(self.points)
        for attr in ("release_node", "loop_node"):
            node = getattr(self, attr)
            setattr(env, attr, None if node is None else int(node) + shift)
        return env

    def to_points(self) -> list:
        """The curve as the flat ``[t, v, shape, curve, ...]`` list the ``bpf``
        widget and a ``"points"`` event both speak."""
        return [x for point in self.points for x in point]

    def set_points(self, points, curve=None) -> "Bpf":
        """Take this curve's points from a break-point list, in place -- the
        other half of `to_points`, and what an editor writes back through. Reads
        every spelling `quads` does."""
        fresh = quads(points, curve)
        if len(fresh) < 2:
            raise ValueError("a curve needs at least two break points")
        self.points = fresh
        for attr in ("release_node", "loop_node"):
            node = getattr(self, attr)
            if node is not None and int(node) >= len(self.points):
                setattr(self, attr, None)
        return self

    def to_inputs(self) -> list:
        """The flat inputs `env_gen` appends -- `Env.to_inputs` of `to_env`, so
        a `Bpf` is played wherever an `Env` is."""
        return self.to_env().to_inputs()

    def duration(self) -> float:
        """The span the curve covers, in **seconds**: its last point's time
        less its first's."""
        return float(self.points[-1][0] - self.points[0][0])
