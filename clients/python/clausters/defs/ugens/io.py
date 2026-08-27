"""What a graph reads from and writes to: buses, replies, disk, feedback.

The bus pair (`in_`/`out`, with their control-rate and replacing forms), the
side-effect UGens that emit an OSC reply or a console post instead of audio (a
def may hold only these and no `out` at all), streaming disk I/O, and the
`local_in`/`local_out` feedback pair.
"""

from ..expr import SynthExpr

from .graph import ChannelList, Ugen


def in_(bus=0.0) -> Ugen:
    """Reads an audio bus (sampled per block)."""
    return Ugen("In", [bus])


def in_ctl(bus=0.0) -> Ugen:
    """Reads a control-bus value, constant over the block."""
    return Ugen("InCtl", [bus])


def _out_channels(kind, bus, signal):
    """One writer per channel on consecutive buses (``bus``, ``bus+1``, …) —
    the point where a channel list becomes buses. The base ``bus`` must be a
    number: a signal bus cannot be offset per channel client-side."""
    if isinstance(bus, bool) or not isinstance(bus, (int, float)):
        raise TypeError(
            f"a multichannel {kind} needs a constant bus to lay channels on "
            f"consecutive buses, got {bus!r}"
        )
    sig = ChannelList(signal)
    return ChannelList(
        [Ugen(kind, [float(bus) + i, s]) for i, s in enumerate(sig.items)]
    )


def out_ctl(bus, signal) -> SynthExpr:
    """Writes ``signal``'s latest per-block value to a **control** ``bus`` — the
    write side of `in_ctl`, so a node reading that bus (via ``/node_map`` or
    `in_ctl`) tracks it. Passes ``signal`` through as its output. A channel
    list writes its channels to consecutive buses."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("OutCtl", bus, signal)
    return Ugen("OutCtl", [bus, signal])


def out(bus, signal) -> SynthExpr:
    """Sums ``signal`` into the audio ``bus`` (output happens only here). A
    channel list writes its channels to consecutive buses: ``out(0,
    dup(sig))`` is a stereo output."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("Out", bus, signal)
    return Ugen("Out", [bus, signal])


def replace_out(bus, signal) -> SynthExpr:
    """Overwrites the audio ``bus`` with ``signal`` instead of summing. A
    channel list overwrites consecutive buses."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("ReplaceOut", bus, signal)
    return Ugen("ReplaceOut", [bus, signal])


# ---- side-effect UGens: reply / observe, no `out` required ----
# These emit OSC replies or console posts on a trigger instead of audio. A
# SynthDef may contain only these and no `out(...)` at all. Pass them as roots
# of the `SynthDef` (they have no consumer to reach them otherwise). A trigger
# fires on a crossing from ``<= 0`` up to ``> 0``.


def send_trig(trig, id=0, value=0.0) -> Ugen:
    """On each trigger of ``trig``, sends ``/node_trigger nodeID id value`` to ``/server_notify``
    clients. Output is silence; pass it as a `SynthDef` root."""
    return Ugen("SendTrig", [trig, id, value])


def send_reply(trig, *values, cmd="/reply", reply_id=-1) -> Ugen:
    """On each trigger of ``trig``, sends the OSC message ``cmd nodeID reply_id
    value…`` to ``/server_notify`` clients (``cmd`` defaults to ``/reply``). ``values``
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


def disk_in(chan=0.0, *, path, loop=False) -> Ugen:
    """Streams a file from disk, one file frame per server sample (no
    resampling — pitch follows the sample-rate ratio). Mono per UGen: ``chan``
    picks the channel, a stereo file is two `disk_in`\\ s. ``loop`` restarts at
    the end of the stream. For a handful of streams, not per-voice (each spawns
    its own I/O thread).

    ``path`` and ``loop`` are **static** fields and are keyword-only —
    ``disk_in(path="take.wav")`` — so the one positional parameter is the one
    input the wire has."""
    return Ugen("DiskIn", [chan], static={"path": str(path), "loop": bool(loop)})


def disk_out(signal, *, path, format="int16") -> Ugen:
    """Streams ``signal`` to a mono WAV at ``path`` (``format`` is ``"int16"``,
    ``"int24"`` or ``"float"``) and passes ``signal`` through as its output.
    Record stereo with two `disk_out`\\ s.

    ``path`` and ``format`` are **static** fields and are keyword-only, so the
    one positional parameter is the one input the wire has.

    It delivers audio out of the graph, so it is a valid def root on its own:
    ``play(disk_out(sig, path=path))`` records **without sounding**. To record
    and hear the same take, route it yourself — ``out(0, disk_out(sig,
    path=path))``, which is what the pass-through output is for."""
    return Ugen("DiskOut", [signal], static={"path": str(path), "format": str(format)})


def local_in(channel=0.0) -> Ugen:
    """Reads synth-private feedback channel ``channel`` (a constant); pairs with
    `local_out` for one-block feedback. ``LocalIn`` must precede its
    ``LocalOut`` — the `SynthDef`'s topological order does that as long
    as the output graph reaches the ``local_in`` before the ``local_out``."""
    return Ugen("LocalIn", [channel])


def local_out(channel, signal) -> Ugen:
    """Writes ``signal`` into synth-private feedback channel ``channel`` (a
    constant); also passes ``signal`` through as its output (so it can be a
    SynthDef output to keep the write in the graph)."""
    return Ugen("LocalOut", [channel, signal])
