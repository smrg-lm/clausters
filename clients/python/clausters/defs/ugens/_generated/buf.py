"""The `buf` builders that mirror the wire, re-exported by `buf`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/buf.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "play_buf",
    "buf_rd",
    "transport_buf_rd",
    "transport_disk_in",
    "buf_wr",
    "osc",
    "oscn",
    "vosc",
    "shaper",
    "sample_rate",
    "buf_frames",
    "buf_sample_rate",
    "buf_rate_scale",
    "buf_channels",
    "buf_dur",
    "rand",
    "irand",
    "exp_rand",
    "lin_rand",
    "n_rand",
    "t_rand",
    "ti_rand",
    "t_exp_rand",
    "coin_gate",
    "rand_id",
    "rand_seed",
    "vosc3",
    "cosc",
    "index",
    "index_l",
    "wrap_index",
    "fold_index",
    "index_in_between",
    "detect_index",
    "degree_to_key",
]


def play_buf(
    bufnum,
    chan=0.0,
    rate=1.0,
    loop=0.0,
    trigger=0.0,
    start_pos=0.0,
    done_action=0.0,
) -> Ugen:
    """Mono buffer player with linear interpolation; `rate` is frames per
    output sample (1.0 = server rate).

    Playing starts at `start_pos` and a rising `trigger` re-cues there,
    so one player is a re-usable voice rather than a one-use node. Without
    `loop`, reaching the end stops it and fires `done_action` (2 frees the
    synth, so a one-shot leaves the tree by itself); a looping player never
    finishes, so the action never fires.
    """
    return Ugen(
        "PlayBuf",
        [bufnum, chan, rate, loop, trigger, start_pos, done_action],
    )


def buf_rd(bufnum, chan, phase, loop=0.0, interpolation=2.0) -> Ugen:
    """Reads a buffer at a `phase` signal in frames (linear interpolation)."""
    return Ugen("BufRd", [bufnum, chan, phase, loop, interpolation])


def transport_buf_rd(
    bufnum,
    chan,
    offset=0.0,
    rate=1.0,
    start=0.0,
    loop=0.0,
) -> Ugen:
    """Reads a buffer **at the transport's position**: `buf_rd` at
    `(position - offset) * rate + start` frames, the phase worked out inside
    the UGen in double precision.

    `offset` is where frame `start` sits on the transport's axis (as
    `transport_pos` takes it), and `rate` the buffer's frames per frame of
    the transport -- `buf_rate_scale` for a buffer at another rate, times a
    playrate. Like `transport_pos` it seeks, loops and holds with the
    transport.

    It is one UGen rather than `buf_rd` with `transport_pos` as its phase
    because a phase handed between UGens is a 32-bit signal, which past about
    six minutes at 48 kHz repeats and drops frames: a long take read that way
    distorts more the further it plays. Here nothing that changes per sample
    is rounded to 32 bits.

    ```python
    take = transport_buf_rd(buf.bufnum, 0, rate=buf_rate_scale(buf.bufnum))
    ```
    """
    return Ugen("TransportBufRd", [bufnum, chan, offset, rate, start, loop])


def transport_disk_in(bufnum, chan, offset=0.0, start=0.0, loop=0.0) -> Ugen:
    """Plays a **fed buffer at the transport's position**: the reader of a
    take an application that holds its engine reads itself and hands over
    through a ring. Over any other buffer it is silent; a buffer of the pool
    is read at the transport's position by `transport_buf_rd`.

    It follows the transport as `transport_buf_rd` does -- `offset` is where
    frame `start` sits on the transport's axis. A locate is a short silence
    and never a stale frame; an owner that cannot keep up plays silence too,
    counted in `ServerStatus.starved_samples`. Silent outside the stream and
    while the transport is stopped.

    Mono, like every buffer reader: `chan` picks one channel of the stream.
    """
    return Ugen("TransportDiskIn", [bufnum, chan, offset, start, loop])


def buf_wr(bufnum, chan, phase, loop, signal) -> Ugen:
    """**Writes** `signal` into a buffer at `phase` (frames) -- `buf_rd`'s
    write-side twin, and stateless in the same way.

    No interpolation: the write lands on the frame the phase names, truncated.
    Spreading one sample over two frames would store a value the signal never
    had, and consecutive writes would fight over the same cells.

    Passes `signal` through as its output, so a chain can go on using what it
    just recorded without a second wire. Out-of-range phases wrap with `loop`
    and write nothing otherwise.
    """
    return Ugen("BufWr", [bufnum, chan, phase, loop, signal])


def osc(bufnum, freq=440.0, phase=0.0) -> Ugen:
    """Interpolating wavetable oscillator. `bufnum` must hold a
    **wavetable-format** buffer (fill it with `buf.gen(...)` and a
    `/buffer_gen` command whose wavetable flag is set); `phase` is an offset in
    radians.
    """
    return Ugen("Osc", [bufnum, freq, phase])


def oscn(bufnum, freq=440.0, phase=0.0) -> Ugen:
    """Non-interpolating oscillator over a **plain** (non-wavetable) buffer;
    rawer and cheaper than `osc`.
    """
    return Ugen("OscN", [bufnum, freq, phase])


def vosc(bufpos, freq=440.0, phase=0.0) -> Ugen:
    """Like `osc` but the buffer number is a signal: reads wavetables
    `bufpos` and `bufpos + 1` and crossfades by the fractional part, so
    sweeping `bufpos` morphs a bank of adjacent tables (allocate them
    contiguously, all the same size).
    """
    return Ugen("VOsc", [bufpos, freq, phase])


def shaper(bufnum, signal) -> Ugen:
    """Waveshaper: maps `signal` (in +-1, clamped) through a transfer table
    in wavetable format (typically a `cheby` `/buffer_gen`); the table's first
    point is `signal = -1`, its last `signal = +1`.
    """
    return Ugen("Shaper", [bufnum, signal])


def sample_rate() -> Ugen:
    """The engine sample rate in Hz, computed once at init (`ir`)."""
    return Ugen("SampleRate", [], rate="ir")


def buf_frames(bufnum) -> Ugen:
    """The number of frames in a buffer, block-constant (`kr`)."""
    return Ugen("BufFrames", [bufnum], rate="kr")


def buf_sample_rate(bufnum) -> Ugen:
    """The buffer's own sample rate (Hz), block-constant (`kr`)."""
    return Ugen("BufSampleRate", [bufnum], rate="kr")


def buf_rate_scale(bufnum) -> Ugen:
    """`file_sr / server_sr`, block-constant (`kr`); feed `play_buf`'s
    `rate` (`buf_rate_scale(buf) * pitch`) to play at the file's true pitch
    without the client knowing either rate.
    """
    return Ugen("BufRateScale", [bufnum], rate="kr")


def buf_channels(bufnum) -> Ugen:
    """The buffer's channel count, block-constant (`kr`)."""
    return Ugen("BufChannels", [bufnum], rate="kr")


def buf_dur(bufnum) -> Ugen:
    """The buffer's duration in seconds (`frames / file_sr`), block-constant
    (`kr`).
    """
    return Ugen("BufDur", [bufnum], rate="kr")


def rand(lo=0.0, hi=1.0) -> Ugen:
    """One uniform random value in `[lo, hi)`, drawn once at synth init and
    held for the node's life (`ir`); `lo`/`hi` must be constants or `ir`.
    """
    return Ugen("Rand", [lo, hi], rate="ir")


def irand(lo=0.0, hi=127.0) -> Ugen:
    """One random integer in `[lo, hi]`, both bounds included (and truncated),
    drawn once at synth init and held.
    """
    return Ugen("IRand", [lo, hi], rate="ir")


def exp_rand(lo=0.01, hi=1.0) -> Ugen:
    """One value between `lo` and `hi` with equal chances per ratio (log-uniform,
    so an octave is as likely as any other), drawn once at synth init and held.
    `lo` and `hi` must have one sign and not be zero.
    """
    return Ugen("ExpRand", [lo, hi], rate="ir")


def lin_rand(lo, hi, minmax=0.0) -> Ugen:
    """One value in `[lo, hi)` leaning towards `lo` (the lower of two uniform
    draws), or towards `hi` when `minmax` is positive, drawn once at synth init
    and held.
    """
    return Ugen("LinRand", [lo, hi, minmax], rate="ir")


def n_rand(lo=0.0, hi=1.0, n=0.0) -> Ugen:
    """One value in `[lo, hi)`, the mean of `n` uniform draws: flat for 1, a
    triangle for 2, nearer a bell the more there are. Drawn once at synth init
    and held.
    """
    return Ugen("NRand", [lo, hi, n], rate="ir")


def t_rand(lo=0.0, hi=1.0, trig=0.0) -> Ugen:
    """A uniform random value in `[lo, hi)`, drawn again at each rising edge of
    `trig` and held between; the first is drawn at the synth's start.
    """
    return Ugen("TRand", [lo, hi, trig])


def ti_rand(lo=0.0, hi=127.0, trig=0.0) -> Ugen:
    """A random integer in `[lo, hi]`, drawn again at each rising edge of `trig`
    and held between; the first is drawn at the synth's start.
    """
    return Ugen("TIRand", [lo, hi, trig])


def t_exp_rand(lo=0.01, hi=1.0, trig=0.0) -> Ugen:
    """A log-uniform random value between `lo` and `hi`, drawn again at each
    rising edge of `trig` and held between; the first is drawn at the synth's
    start.
    """
    return Ugen("TExpRand", [lo, hi, trig])


def coin_gate(prob=0.5, trig=0.0) -> Ugen:
    """Lets each rising edge of `trig` through with probability `prob` (the
    trigger's own value, for that sample), and outputs 0 otherwise.
    """
    return Ugen("CoinGate", [prob, trig])


def rand_id(id=0.0) -> Ugen:
    """Chooses which of the server's 64 random generators this synth's random
    UGens draw from, `id` 0 to 63 (0 until it runs). Synths drawing from
    different generators are independent streams: a reseed of one leaves the
    others alone, and they can run side by side in a parallel group. Outputs 0.
    """
    return Ugen("RandID", [id])


def rand_seed(trig=0.0, seed=56789.0) -> Ugen:
    """At each rising edge of `trig`, reseeds the generator this synth draws from
    with `seed`, so every synth drawing from it starts the same sequence again
    -- how a group's randomness is restarted on purpose. At the initial rate it
    reseeds once, when the synth starts. A def of its own does nothing else:
    it is a root, and is not wrapped in an `out`. Outputs 0.
    """
    return Ugen("RandSeed", [trig, seed])


def vosc3(bufpos, freq1=110.0, freq2=220.0, freq3=440.0) -> Ugen:
    """`vosc` read at three frequencies and summed: three detuned voices of one
    morphing timbre.
    """
    return Ugen("VOsc3", [bufpos, freq1, freq2, freq3])


def cosc(bufnum, freq=440.0, beats=0.5) -> Ugen:
    """`osc` twice, at `freq + beats/2` and `freq - beats/2`, summed: two voices
    beating `beats` times a second, a chorus from one wavetable. The peak is
    twice the table's.
    """
    return Ugen("COsc", [bufnum, freq, beats])


def index(bufnum, signal) -> Ugen:
    """The buffer as a table: its value at `signal` truncated, the index clipped
    to the table.
    """
    return Ugen("Index", [bufnum, signal])


def index_l(bufnum, signal) -> Ugen:
    """The buffer as a table read at a fractional index, linearly between the two
    values either side, the index clipped to the table.
    """
    return Ugen("IndexL", [bufnum, signal])


def wrap_index(bufnum, signal) -> Ugen:
    """`index` with the index wrapped around the table."""
    return Ugen("WrapIndex", [bufnum, signal])


def fold_index(bufnum, signal) -> Ugen:
    """`index` with the index reflected at the table's ends."""
    return Ugen("FoldIndex", [bufnum, signal])


def index_in_between(bufnum, signal) -> Ugen:
    """The inverse of `index_l` over an ascending table: the fractional index at
    which the table reaches `signal`.
    """
    return Ugen("IndexInBetween", [bufnum, signal])


def detect_index(bufnum, signal) -> Ugen:
    """The index of the first table value equal to `signal`, or -1."""
    return Ugen("DetectIndex", [bufnum, signal])


def degree_to_key(bufnum, signal, octave=12.0) -> Ugen:
    """A scale degree to a key: the buffer holds one octave of the scale, and
    degree `signal` (floored) is its step plus `octave` per whole octave up or
    down. Degree -1 is the last step an octave down.
    """
    return Ugen("DegreeToKey", [bufnum, signal, octave])
