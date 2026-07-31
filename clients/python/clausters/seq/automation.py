"""Automation: a break-point control curve as a control vector (buffer).

An `Automation` places a break-point curve on the timeline that drives one or
more ``(node, control)`` targets. It is rendered as a **control vector**: the
curve is discretized on the server into a control buffer (``/b_gen "env"``,
evaluated through the same envelope-shape math the ``EnvGen`` UGen plays), and a
small control synth reads that buffer onto a control bus which the targets
follow via ``/n_map``. The stored curve is an `Env` (the same object the ``bpf``
editor round-trips through `env_to_points`/`points_to_env`).

Two phases keep the clock thread unblocked — a routine must **never** block:

- `Automation.prepare` allocates and fills the buffer and allocates the bus.
  Blocking is fine here (it runs at setup, off the clock thread); in NRT it
  scores the ``/b_alloc``/``/b_gen`` at time 0.
- `Automation.play` — the Timeline-item hook — only *schedules* the lane synth,
  the ``/n_map``\\ s and the ``/n_free`` (all non-blocking). In NRT, where every
  command is scored in order, `play` self-prepares.
"""

from ..base.main import main
from ..defs.buffer import Buffer
from ..defs.bus import Bus
from ..defs.node import AddAction, Node, ROOT_NODE_ID
from ..defs.synthdef import SynthDef
from ..defs.ugens import (
    buf_frames,
    control,
    env_to_points,
    out_ctl,
    play_buf,
    points_to_env,
    sample_rate,
    _resolve_curve,
)

#: The internal control-synth def name and the default curve resolution.
LANE_DEF = "clausters.auto_lane"
DEFAULT_FRAMES = 1024


def auto_lane_def() -> SynthDef:
    """The internal control synth that plays a control buffer onto a control bus
    over ``dur`` seconds. The playback rate is derived from the buffer length and
    the engine sample rate, so the whole buffer spans ``dur`` regardless of the
    sample rate; the client passes only ``buf``, ``bus`` and ``dur``."""
    buf = control("buf", 0.0, "ir")
    bus = control("bus", 0.0, "ir")
    dur = control("dur", 1.0, "ir")
    rate = buf_frames(buf) / (dur * sample_rate())
    sig = play_buf(buf, 0.0, rate, 0.0)
    return SynthDef(LANE_DEF, out_ctl(bus, sig))


def add_automation_def(server, *, wait: bool = True):
    """Register the automation lane def on ``server`` once (idempotent)."""
    if getattr(server, "_automation_def", False):
        return
    auto_lane_def().send(server, wait=wait)
    server._automation_def = True


def _norm_targets(target):
    """A single ``(node, control)`` pair or an iterable of them → a list."""
    if target is None:
        return []
    if (isinstance(target, tuple) and len(target) == 2
            and not isinstance(target[0], (list, tuple))):
        return [target]
    return [tuple(t) for t in target]


def _env_gen_args(env):
    """The flat ``/b_gen "env"`` argument list: ``level0`` then a
    ``(level, time, shape, curve)`` quad per segment (times relative — only
    their proportions matter, playback maps them onto real time)."""
    args = [float(env.levels[0])]
    for k in range(len(env.times)):
        shape, curve = _resolve_curve(env.curves[k])
        args += [float(env.levels[k + 1]), float(env.times[k]), int(shape), float(curve)]
    return args


class Automation:
    """A control-automation lane: a break-point curve (`Env`) driving one or
    more ``(node, control)`` targets, rendered as a control buffer read onto a
    control bus. Editable through `to_points`/`from_points` (the ``bpf`` widget's
    flat ``[time, value, shape, curve, ...]`` form, in real control units).

    Usage::

        auto = Automation.from_points(
            [(0, 200.0, "lin", 0), (2, 4000.0, "exp", 0)],
            target=(synth, "cutoff"))
        auto.prepare(server)          # RT: at setup, off the clock thread
        timeline.add(0, auto)         # played by the Playhead as a Timeline item
    """

    def __init__(self, env, target, *, name=None, frames: int = DEFAULT_FRAMES):
        self.env = env
        self.targets = _norm_targets(target)
        self.name = name or (self.targets[0][1] if self.targets else "automation")
        self.frames = int(frames)
        self.buf = None
        self.bus = None
        #: the lane synth of the last `play` and its destination, so `stop`
        #: can interrupt the sweep early.
        self.node = None
        self._playing_on = None

    @classmethod
    def from_points(cls, points, target, *, name=None, frames: int = DEFAULT_FRAMES,
                    **env_kwargs) -> "Automation":
        """Build from a ``bpf`` breakpoint list ``[(time, value, shape, curve), …]``
        (or the flat ``[t, v, shape, curve, …]`` a ``"points"`` event carries).
        Times are in beats; values in the target control's real units."""
        flat = points if points and not isinstance(points[0], (list, tuple)) else [
            x for p in points for x in p
        ]
        return cls(points_to_env(flat, **env_kwargs), target, name=name, frames=frames)

    def to_points(self) -> list:
        """The curve as the ``bpf`` flat breakpoint list ``[t, v, shape, curve, …]``."""
        return env_to_points(self.env)

    def duration(self) -> float:
        """The curve's length in beats (the sum of its segment times)."""
        return float(sum(self.env.times))

    def prepare(self, server, *, wait: bool = True) -> "Automation":
        """Allocate and fill the control buffer and allocate the bus. Call once,
        at setup (blocking in RT; scored at time 0 in NRT). ``play`` self-prepares
        in NRT."""
        add_automation_def(server, wait=wait)
        if self.buf is None:
            self.buf = Buffer.alloc(self.frames, 1, wait=wait, server=server)
        self.buf.gen("env", *_env_gen_args(self.env), wait=wait)
        if self.bus is None:
            self.bus = Bus.control(server=server)
        return self

    def play(self, destination):
        """Timeline-item hook and interactive trigger: schedule the lane synth,
        ``/n_map`` the targets, and free the synth after the curve's duration.
        Non-blocking. Self-prepares only in NRT (where it is scored).

        Two timing regimes, chosen by context like an `Event`'s. **Inside a
        routine** everything goes out as timetagged bundles at the routine's
        exact logical beat. **Outside any clock** (an interactive
        ``play(auto)``) the lane starts immediately and frees itself on wall
        time, the curve's beats read as seconds (tempo 1.0)."""
        if self.buf is None or self.bus is None:
            if getattr(destination.interface, "time_mode", "unix") != "score":
                raise RuntimeError(
                    "Automation.play: call prepare(server) first in RT "
                    "(allocating/filling a buffer must not block the clock thread)")
            self.prepare(destination)

        clock = getattr(main.current_tt, "clock", None)
        dur_beats = self.duration()
        dur_secs = dur_beats if clock is None else dur_beats / clock.tempo

        node = destination.nodes.alloc()
        self.node, self._playing_on = node, destination
        s_new = ("/s_new", LANE_DEF, node, int(AddAction.HEAD), int(ROOT_NODE_ID),
                 "buf", self.buf.bufnum, "bus", self.bus.index, "dur", dur_secs)
        maps = [("/n_map", tnode.id if hasattr(tnode, "id") else int(tnode),
                 ctl, self.bus.index) for tnode, ctl in self.targets]
        if clock is None:
            # No clock in context: immediate lane, self-freeing on wall time.
            destination.send_msg(*s_new)
            for m in maps:
                destination.send_msg(*m)
            destination.send_bundle_after(dur_secs, ("/n_free", node))
            return node
        destination.send_bundle(s_new)
        for m in maps:
            destination.send_bundle(m)
        destination.send_bundle(("/n_free", node), delay_beats=dur_beats)
        return node

    def stop(self):
        """Interrupt the sweep **now**: free the lane synth of the last `play`,
        so the curve stops advancing and the mapped controls hold their last
        value. A no-op when nothing is playing; the end-of-curve free already
        scheduled still arrives and is harmless."""
        if self.node is not None and self._playing_on is not None:
            Node(self.node, self._playing_on).free()
        self.node = self._playing_on = None

    def free(self, server):
        """Return the buffer and bus to their allocators."""
        if self.buf is not None:
            self.buf.free()
            self.buf = None
        if self.bus is not None:
            server.control_buses.free(self.bus)
            self.bus = None
