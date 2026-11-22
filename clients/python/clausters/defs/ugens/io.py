"""What a graph reads from and writes to: buses, replies, disk, feedback.

The bus pair (`in_`/`out`, with their control-rate and replacing forms), the
side-effect UGens that emit an OSC reply or a console post instead of audio (a
def may hold only these and no `out` at all), streaming disk I/O, and the
`local_in`/`local_out` feedback pair.
"""

from ..expr import SynthExpr

from .graph import ChannelList, Ugen, _Node
from ._generated.io import *  # noqa: F403 -- the builders that mirror the wire


def _out_channels(kind, bus, signal):
    """One writer per channel on consecutive buses (``bus``, ``bus+1``, ...) --
    the point where a channel list becomes buses. A constant ``bus`` is
    offset here; a control (or any single-channel node) takes the first
    channel itself and an ``Add`` of the offset for each one after, which the
    server reads as a bus it can still order the node by."""
    if isinstance(bus, bool) or not isinstance(bus, (int, float, _Node)):
        raise TypeError(
            f"a multichannel {kind} lays its channels on consecutive buses "
            f"from one bus -- a number or a single-channel node -- got {bus!r}"
        )
    sig = ChannelList(signal)
    if isinstance(bus, _Node):
        return ChannelList(
            [Ugen(kind, [bus if i == 0 else bus + float(i), s])
             for i, s in enumerate(sig.items)]
        )
    return ChannelList(
        [Ugen(kind, [float(bus) + i, s]) for i, s in enumerate(sig.items)]
    )


def out_ctl(bus, signal) -> SynthExpr:
    """Writes ``signal``'s latest per-block value to a **control** ``bus`` -- the
    write side of `in_ctl`, so a node reading that bus (via ``/node_map`` or
    `in_ctl`) tracks it. Passes ``signal`` through as its output. A channel
    list writes its channels to consecutive buses, from a number or from a
    control alike."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("OutCtl", bus, signal)
    return Ugen("OutCtl", [bus, signal])


def out(bus, signal) -> SynthExpr:
    """Sums ``signal`` into the audio ``bus`` (output happens only here). A
    channel list writes its channels to consecutive buses: ``out(0,
    dup(sig))`` is a stereo output, and so is ``out(control("out", 0.0),
    dup(sig))``, which follows the control wherever a note sets it."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("Out", bus, signal)
    return Ugen("Out", [bus, signal])


def replace_out(bus, signal) -> SynthExpr:
    """Overwrites the audio ``bus`` with ``signal`` instead of summing. A
    channel list overwrites consecutive buses, from a number or from a
    control alike."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("ReplaceOut", bus, signal)
    return Ugen("ReplaceOut", [bus, signal])


# ---- side-effect UGens: reply / observe, no `out` required ----
# These emit OSC replies or console posts on a trigger instead of audio. A
# SynthDef may contain only these and no `out(...)` at all. Pass them as roots
# of the `SynthDef` (they have no consumer to reach them otherwise). A trigger
# fires on a crossing from ``<= 0`` up to ``> 0``.


def send_reply(trig, *values, cmd="/reply", reply_id=-1) -> Ugen:
    """On each trigger of ``trig``, sends the OSC message ``cmd nodeID reply_id
    value...`` to ``/server_notify`` clients (``cmd`` defaults to ``/reply``). ``values``
    is the arbitrary-arity payload. Output is silence; pass it as a `SynthDef`
    root."""
    return Ugen("SendReply", [trig, reply_id, *values], label=cmd)


def poll(trig, signal, trig_id=-1, *, label="poll") -> Ugen:
    """On each trigger of ``trig``, posts ``label: value`` (the ``signal``
    value) to the server console and, when ``trig_id >= 0``, also sends ``/node_trigger
    nodeID trig_id value``. ``signal`` passes through the output, so ``poll``
    can sit mid-chain.

    ``label`` is a **static** field and is keyword-only, so the positional
    parameters are the wire's three inputs in the wire's order."""
    return Ugen("Poll", [trig, signal, trig_id], label=label)

# ---- streaming disk I/O (self-contained: one I/O thread + ring each) ----


def disk_in(chan=0.0, *, path, loop=False, start=0) -> Ugen:
    """Streams a file from disk, one file frame per server sample (no
    resampling -- pitch follows the sample-rate ratio). Mono per UGen: ``chan``
    picks the channel, a stereo file is two `disk_in`\\ s. ``start`` is the
    frame of the file the stream begins at (its top by default), and ``loop``
    restarts at the file's top at the end of the stream. For a handful of
    streams, not per-voice (each spawns its own I/O thread).

    ``path``, ``loop`` and ``start`` are **static** fields and are
    keyword-only -- ``disk_in(path="take.wav")`` -- so the one positional
    parameter is the one input the wire has."""
    static = {"path": str(path), "loop": bool(loop)}
    if start:
        static["start"] = int(start)
    return Ugen("DiskIn", [chan], static=static)


def disk_out(signal, *, path, format="int16") -> Ugen:
    """Streams ``signal`` to a mono sound file at ``path`` (``format`` is
    ``"int16"``, ``"int24"`` or ``"float"``) and passes ``signal`` through as
    its output: a WAV, closed as an RF64 if the take outgrew 4 GB, or a W64
    or an RF64 when the path ends in ``.w64`` or ``.rf64``. Record stereo with
    two `disk_out`\\ s.

    ``path`` and ``format`` are **static** fields and are keyword-only, so the
    one positional parameter is the one input the wire has.

    It delivers audio out of the graph, so it is a valid def root on its own:
    ``play(disk_out(sig, path=path))`` records **without sounding**. To record
    and hear the same take, route it yourself -- ``out(0, disk_out(sig,
    path=path))``, which is what the pass-through output is for."""
    return Ugen("DiskOut", [signal], static={"path": str(path), "format": str(format)})




def send_peak_rms(*signals, reply_rate=20.0, peak_lag=3.0, cmd="/reply", reply_id=-1) -> Ugen:
    """``reply_rate`` times a second, sends ``cmd nodeID reply_id`` and each
    signal's peak and RMS over the interval -- the peak held and falling back
    by ``peak_lag`` seconds to -60 dB, a meter's ballistics. Up to eight
    signals. Output is silence; pass it as a `SynthDef` root."""
    return Ugen("SendPeakRMS", [reply_rate, peak_lag, reply_id, *signals], label=cmd)
