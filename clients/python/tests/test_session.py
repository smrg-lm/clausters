"""C5 leftover: ergonomic defaults without global state. A Session bundles a
Server and a clock explicitly, and several coexist (no globals)."""

import pytest

from clausters import Session
from clausters.base import MonotonicTimebase, TempoClock
from clausters.defs import Server
from clausters.seq import Pbind, Pseq
from clausters.defs import Group, Synth


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def test_session_and_default_are_the_same_kind_of_environment():
    # The default session and an explicit Session share one base: both are
    # Environments (server + isolated random context). main *is* a session.
    from clausters import main, default_session
    from clausters.base import Environment

    assert issubclass(Session, Environment)
    assert isinstance(main, Environment)
    assert default_session is main
    for env in (main, Session.nrt(tempo=1.0)):
        assert hasattr(env, "server") and hasattr(env, "seed") and hasattr(env, "rng")


def test_nrt_session_plays_and_renders():
    _embed_or_skip()
    s = Session.nrt(tempo=2.0)
    s.play(Pbind(instrument="default", freq=Pseq([262.0, 330.0, 392.0, 523.0]),
                 dur=0.5, amp=0.2))
    try:
        _st0 = s.render()
        samples, frames = _st0.samples, _st0.frames
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")
    assert frames > 0
    assert max(abs(x) for x in samples) > 0.0


def test_nrt_render_with_workers_is_bit_identical():
    # A parallel group of independent voices: `workers` must only change
    # wall-clock time, never the samples.
    _embed_or_skip()
    from clausters.defs import SynthDef, control, out, sine

    def build():
        s = Session.nrt(tempo=1.0)
        server = s.server
        SynthDef(
            "par_voice", out(0.0, sine(control("freq", 330.0)) * 0.1)).send(server)
        band = Group(server=server)
        server.send_msg("/group_parallel", band.id, 1)
        voices = [Synth("par_voice", {"freq": 220.0 * (i + 1)},
                               target=band.id, server=server) for i in range(4)]

        def score():
            yield 0.5
            server.send_bundle(*[("/node_free", v.id) for v in voices])

        from clausters.base.stream import Routine
        s.clock.play(Routine(score))
        return s

    try:
        a = build().render(channels=2)
        b = build().render(channels=2, workers=2)
        seq, frames = a.samples, a.frames
        par, frames2 = b.samples, b.frames
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")
    assert frames == frames2 and frames > 0
    assert list(seq) == list(par)


def test_boot_workers_becomes_the_cli_flag(monkeypatch):
    # Server().boot(workers=N) must launch `clausters --workers N` (before any
    # explicit server_args, so those stay the escape hatch that wins).
    import clausters.launch as launch
    from clausters.launch import DEFAULT_PORT

    captured = {}

    class FakeProcess:
        # The address is a class attribute, as the real launcher's is: the
        # binary takes no port flag, and `boot` reads it off the class to
        # refuse a handle pointing where a booted server cannot be.
        host, port = "127.0.0.1", DEFAULT_PORT

        def __init__(self, options=None, **kwargs):
            captured.update(kwargs)
            self.shm = None

        def start(self):
            return self

    monkeypatch.setattr(launch, "ServerProcess", FakeProcess)
    server = Server(transport="udp").boot(
        workers=3, server_args=("--tcp",), adopt_default=False)
    try:
        assert captured["extra_args"] == ["--workers", "3", "--tcp"]
    finally:
        server.interface.close()
    # None emits no flag: the server keeps its config-file default.
    Server(transport="udp").boot(adopt_default=False).interface.close()
    assert captured["extra_args"] == []


def _embed_session_or_skip():
    """An embedded live session, or skip if the embed library is unusable here
    (not built with embed,realtime, or no audio device available)."""
    _embed_or_skip()
    from clausters import Session
    from clausters.errors import LibraryError, ServerError
    try:
        return Session.embed(tempo=4.0, latency=0.1)
    except (LibraryError, ServerError, OSError) as e:
        pytest.skip(f"embedded server not available: {e}")


def test_embed_session_drives_in_process_server():
    # The embedded server is just another OSC destination: the same Session /
    # Pbind API drives it, request/reply works through the embed interface, and
    # the in-process engine actually advances.
    s = _embed_session_or_skip()
    try:
        embed = s.server.interface.server          # the Clausters handle
        assert s.server.status()[0] == 1           # request/reply over embed

        s.play(Pbind(instrument="default", freq=Pseq([440.0, 550.0, 660.0]),
                     dur=0.25, amp=0.2))
        c0 = embed.clock
        s.run(0.6)
        assert embed.clock > c0                     # engine ran in-process
        assert s.server.query_tree().id == 0        # another reply path
    finally:
        s.close()


def test_embed_session_anchors_to_the_sample_clock_by_default():
    # Like live, an embed session sample-locks out of the box — but through a
    # direct in-process read of the shared counter (EmbedSampleClock), with no
    # UDP tracker: no socket, no round trips, no timeout to burn.
    from clausters.base.timebase import SampleClockTimebase
    from clausters.defs.clocksync import EmbedSampleClock

    s = _embed_session_or_skip()
    try:
        assert isinstance(s.clock.timebase, SampleClockTimebase)
        assert isinstance(s.clock._sample_clock, EmbedSampleClock)
        # the timebase reads the handle's counter directly
        embed = s.server.interface.server
        assert s.clock.timebase.current_sample() == pytest.approx(embed.clock, abs=8192)
        # lock_to is idempotent: a manual lock keeps the in-process reader
        sc = s.clock._sample_clock
        assert s.lock_to_server() is s
        assert s.clock._sample_clock is sc
    finally:
        s.close()


def test_embed_session_is_independent_from_others():
    # No global state: an embedded session coexists with an offline one, each
    # with its own server and clock.
    s = _embed_session_or_skip()
    try:
        b = Session.nrt(tempo=1.0)
        assert s.server is not b.server
        assert s.clock is not b.clock
        b.close()
    finally:
        s.close()


def test_two_sessions_are_independent():
    _embed_or_skip()
    a = Session.nrt(tempo=1.0)
    b = Session.nrt(tempo=1.0)
    a.play(Pbind(instrument="default", freq=Pseq([100.0, 200.0]), dur=0.5))
    b.play(Pbind(instrument="default", freq=Pseq([1000.0, 2000.0, 3000.0]), dur=0.5))
    a.clock.render()
    b.clock.render()

    # each session kept its own score (2 notes vs 3, each = /synth_new + /node_free)
    assert len(a.server.interface.score.bundles) == 2 * 2
    assert len(b.server.interface.score.bundles) == 3 * 2
    # and they are genuinely separate objects — no shared global
    assert a.server is not b.server
    assert a.clock is not b.clock


def test_lock_to_offline_session_is_a_noop():
    # An NRT (score) server has no live clock; lock_to must leave the clock on
    # wall-clock OSC time (and not raise), so offline scripts keep working.
    s = Session.nrt(tempo=2.0)
    assert isinstance(s.clock.timebase, MonotonicTimebase)
    assert s.lock_to_server() is s            # chainable, no-op here
    assert isinstance(s.clock.timebase, MonotonicTimebase)
    assert s.clock._sample_clock is None
    s.close()


def test_lock_to_unreachable_master_falls_back_to_wall_clock():
    # No server is listening on this port: lock_to detects no master within the
    # timeout and stays on wall-clock OSC time (the "client with no Clausters
    # server" case), rather than raising.
    server = Server("127.0.0.1", 59999)
    clock = TempoClock(tempo=1.0)
    assert clock.lock_to(server, timeout=0.2) is clock
    assert isinstance(clock.timebase, MonotonicTimebase)
    assert clock._sample_clock is None
    clock.close()
    server.close()


def test_quant_snaps_to_the_clock_grid():
    # quant delays the start to the next multiple of `quant` beats on the
    # clock's own grid (beats() is the logical beat while not running).
    clock = TempoClock(tempo=2.0)
    clock._logical_beat = 0.0
    assert clock._quant_delay(4) == 0.0          # on a boundary -> now
    clock._logical_beat = 3.5
    assert clock._quant_delay(4) == pytest.approx(0.5)
    clock._logical_beat = 5.0
    assert clock._quant_delay(2) == pytest.approx(1.0)   # next even beat is 6
    assert clock._quant_delay(None) == 0.0       # no quant -> now
    assert clock._quant_delay(0) == 0.0


def test_quant_on_a_joined_wall_clock_transport():
    import time as _t

    # A wall-clock client joined to a transport whose origin was ~1 s ago at
    # tempo 2 bps -> ~2 beats elapsed; the next bar (quant 4) is ~2 beats off.
    clock = TempoClock(tempo=2.0)
    clock._transport = ("wall", _t.time() - 1.0, 2.0)
    assert 1.8 < clock._grid_beat() < 2.2
    assert 1.8 < clock._quant_delay(4) < 2.2


if __name__ == "__main__":
    import traceback

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except BaseException as e:  # noqa: BLE001
                kind = type(e).__name__
                skip = kind in ("Skipped", "OutcomeException")
                print(f"{'skip' if skip else 'FAIL'} {name}: {e}")
                if not skip:
                    traceback.print_exc()


# ---- the catalog contrast: ugens.py against the server's own /ugen_query ----

# The kinds whose Python signature does not line up with the wire order, so the
# positional contrast below cannot apply to them. They are split by *why*,
# because the two halves have very different standing — and the test asserts the
# union is exact, so a new divergence has to be declared here on purpose rather
# than silently dropping a kind from the check.

# Forced by the wire putting a variadic run last: Python cannot have a plain
# positional parameter after `*args`, so the ergonomic order is the only one
# available. These will not change.
_WIRE_ORDER_FORCED = {
    "EnvGen": "the Env comes first in Python, its array last on the wire",
    "SendReply": "reply_id follows trig on the wire, but must be keyword-only here",
    "Dseq": "repeats leads on the wire; the value list is the leading argument here",
    "Drand": "repeats leads on the wire; the value list is the leading argument here",
    "Dxrand": "repeats leads on the wire; the value list is the leading argument here",
    "Dshuf": "repeats leads on the wire; the value list is the leading argument here",
}

# Not forced: these take their **static** (non-signal) fields as ordinary
# positional parameters, interleaved with real inputs — where `fft` and `conv`
# put theirs behind a `*` and line up with the wire exactly. Moving them behind
# a `*` would fix the divergence and shrink this list, but it breaks the client
# API (`poll(t, s, "label")` and friends stop working), so it is deliberately
# deferred rather than done as a side effect of M30. `Poll` is the worst: its
# static `label` sits *between* two genuine inputs.
_WIRE_ORDER_STATIC_FIELDS_POSITIONAL = {
    "Poll": "label is a static field sitting between the wire inputs",
    "DiskIn": "path/loop are static fields, only chan is an input",
    "DiskOut": "path/format are static fields, only signal is an input",
    "PV_Kernel": "mag/phase are static fields, the wire takes chain + params",
}

# Not a divergence in the same sense: these rows end in a `chan` index the
# *builder* fills, once per channel, because a UGen has one output and a panner
# has two. The caller never passes it, so the Python signature is the wire's
# minus its last input — by construction, for every row of the family.
_WIRE_CHANNEL_IS_THE_BUILDERS = {
    "Pan2": "built twice, one row per output channel",
    "LinPan2": "built twice, one row per output channel",
    "Balance2": "built twice, one row per output channel",
    "Rotate2": "built twice, one row per output channel",
    "MidSide": "built twice, one row per output channel",
    "StereoWidth": "built twice, one row per output channel",
    "PanAz": "built numchans times; numchans leads in Python, trails on the wire",
}

_WIRE_ORDER_DIFFERS = {
    **_WIRE_ORDER_FORCED,
    **_WIRE_ORDER_STATIC_FIELDS_POSITIONAL,
    **_WIRE_CHANNEL_IS_THE_BUILDERS,
}


def _python_callables_by_kind():
    """Maps each wire kind to the `clausters.defs.ugens` callable that builds
    it, read from the ``Ugen("Kind", ...)`` literal in the source rather than
    guessed from the function name (``in_`` -> ``In``, ``oscn`` -> ``OscN``)."""
    import ast
    import inspect

    from clausters.defs import ugens as U

    out = {}
    for name, fn in vars(U).items():
        if name.startswith("_") or not inspect.isfunction(fn):
            continue
        try:
            tree = ast.parse(inspect.getsource(fn).lstrip())
        except (OSError, SyntaxError):
            continue
        for node in ast.walk(tree):
            if (isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Name) and node.func.id == "Ugen"
                    and node.args and isinstance(node.args[0], ast.Constant)):
                out.setdefault(node.args[0].value, fn)
    return out


def test_ugen_catalog_matches_the_python_callables():
    """`/ugen_query` is the server's own catalog; `clausters.defs.ugens` is the
    client's hand-written mirror of it. This is what keeps the two from
    drifting: for every kind whose Python signature maps 1:1 onto the wire, the
    input names and defaults must agree exactly."""
    import inspect
    import struct

    s = _embed_session_or_skip()
    try:
        catalog = s.server.query_ugens()
        assert catalog, "a synth-enabled server must report a catalog"
        by_kind = _python_callables_by_kind()

        checked = 0
        for u in catalog:
            fn = by_kind.get(u.name)
            if fn is None or u.name in _WIRE_ORDER_DIFFERS:
                continue
            params = [p for p in inspect.signature(fn).parameters.values()
                      if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)]
            assert [p.name for p in params] == [i.name for i in u.inputs], (
                f"{u.name}: Python names {[p.name for p in params]} but the "
                f"server names {[i.name for i in u.inputs]}"
            )
            for p, i in zip(params, u.inputs):
                if p.default is inspect.Parameter.empty:
                    continue
                # `None` is a client-side sentinel, not a value: the filter
                # builders take `rq=None` so that `q=` can be given instead and
                # the pair resolved together (see `_resonance`). The name
                # contrast above still covers those slots; only the number is
                # unavailable here.
                if p.default is None:
                    continue
                # The server's defaults are f32 and arrive widened, so 0.1
                # comes back as 0.10000000149...: compare at f32 precision.
                as_f32 = struct.unpack("f", struct.pack("f", float(p.default)))[0]
                assert as_f32 == i.default, (
                    f"{u.name}.{i.name}: Python default {p.default} != "
                    f"server default {i.default}"
                )
            checked += 1

        assert checked > 25, f"only contrasted {checked} kinds"

        # The exception list must be exact: every entry still exists in both
        # catalogs, so a renamed or removed kind cannot leave a stale excuse
        # behind that silently drops a UGen from the contrast.
        kinds = {u.name for u in catalog}
        for name in _WIRE_ORDER_DIFFERS:
            assert name in kinds, f"{name} is no longer in the server catalog"
            assert name in by_kind, f"{name} has no Python callable any more"
    finally:
        s.close()
