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
    """One writer per channel on consecutive buses (``bus``, ``bus+1``, …) --
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
    """Writes ``signal``'s latest per-block value to a **control** ``bus`` -- the
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


def meter(signal, decay=20.0, hold=0.0) -> Ugen:
    """A **level with the ballistics a person can read**: instantaneous attack,
    a fall of ``decay`` decibels per second, and a peak held ``hold`` seconds
    before it starts falling.

    One block is one measurement -- the block's own peak in, the whole block out
    at what the meter now reads -- which is what a meter is and what a control
    bus carries, so a finer answer would be samples nothing can read. The rules
    are the shared crate's (`measure::Ballistics`), so every meter drawn
    anywhere falls at the same rate.

    It **emits** the value rather than writing it anywhere, so what to do with
    it stays yours: a control bus per channel, a `send_reply`, or a signal that
    drives something.
    """
    return Ugen("Meter", [signal, decay, hold])


def clip_count(signal, ceiling=1.0, run=3) -> Ugen:
    """**How many times the signal was flattened**, counted since the pass
    began: a run of ``run`` consecutive samples at or over ``ceiling`` is one
    over, however long it goes on.

    The count and not a flag, because the flag is a *reader's* state -- a meter's
    red lamp stays lit until a hand puts it out, and two windows watching one
    bus each have their own -- while the count is the signal's. Pair it with
    ``out_ctl`` and hand the bus to a ``meter`` widget's ``clip``: the widget
    differences the count and lights up on an over it has not seen.

    A single sample at full scale is **not** clipping. The engine runs in
    floating point, so a sample at 1.0 destroys nothing until the signal is
    converted or reaches a converter; what a red lamp reports is a waveform that
    was flattened, and the signature of that is a run. Three is the field's
    number, and ``run=1`` is the pessimistic meter that lights on any sample
    touching full scale.

    It runs per **sample**, unlike `meter`: the run is the whole of what
    distinguishes a flattened peak from a peak that legitimately reached full
    scale, and a block peak has already lost it.
    """
    return Ugen("ClipCount", [signal, ceiling, run])


def true_peak(signal, decay=20.0, hold=0.0) -> Ugen:
    """A **meter's level over the reconstructed signal**: the same ballistics as
    `meter` -- instantaneous attack, a fall of ``decay`` decibels per second, a
    peak held ``hold`` seconds -- but what goes in is the block's **true peak**
    rather than its largest sample.

    The signal between two samples can reach past both: a tone sampled so that
    every sample sits at full scale can be three decibels over it in between,
    and every converter sees that. The reconstruction is the filter ITU-R
    BS.1770-4 Annex 2 specifies, so the reading is in **dBTP** and is never
    below what `meter` reads off the same signal. Hand the bus to a ``meter``
    widget with ``rate="control"`` and ``peak="true"``, which reads it against
    the -1 dBTP ceiling rather than full scale.
    """
    return Ugen("TruePeak", [signal, decay, hold])


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
    resampling -- pitch follows the sample-rate ratio). Mono per UGen: ``chan``
    picks the channel, a stereo file is two `disk_in`\\ s. ``loop`` restarts at
    the end of the stream. For a handful of streams, not per-voice (each spawns
    its own I/O thread).

    ``path`` and ``loop`` are **static** fields and are keyword-only --
    ``disk_in(path="take.wav")`` -- so the one positional parameter is the one
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
    and hear the same take, route it yourself -- ``out(0, disk_out(sig,
    path=path))``, which is what the pass-through output is for."""
    return Ugen("DiskOut", [signal], static={"path": str(path), "format": str(format)})


def local_in(channel=0.0) -> Ugen:
    """Reads synth-private feedback channel ``channel`` (a constant); pairs with
    `local_out` for one-block feedback. ``LocalIn`` must precede its
    ``LocalOut`` -- the `SynthDef`'s topological order does that as long
    as the output graph reaches the ``local_in`` before the ``local_out``."""
    return Ugen("LocalIn", [channel])


def local_out(channel, signal) -> Ugen:
    """Writes ``signal`` into synth-private feedback channel ``channel`` (a
    constant); also passes ``signal`` through as its output (so it can be a
    SynthDef output to keep the write in the graph)."""
    return Ugen("LocalOut", [channel, signal])
