"""The `filter` builders that mirror the wire, re-exported by `filter`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/filter.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "lpf",
    "hpf",
    "one_pole",
    "one_zero",
    "leak_dc",
    "integrator",
    "lag",
    "lag_ud",
    "buf_delay_n",
    "buf_delay_l",
    "buf_delay_c",
    "buf_comb_n",
    "buf_comb_l",
    "buf_comb_c",
    "buf_allpass_n",
    "buf_allpass_l",
    "buf_allpass_c",
    "delay1",
    "delay2",
    "lpz1",
    "hpz1",
    "lpz2",
    "hpz2",
    "bpz2",
    "brz2",
    "slope",
    "flip",
    "two_zero",
    "fos",
    "sos",
    "ramp",
    "slew",
    "two_pole",
    "apf",
    "ringz",
    "formlet",
    "mid_eq",
    "b_low_pass",
    "b_hi_pass",
    "b_all_pass",
    "b_band_pass",
    "b_band_stop",
    "b_peak_eq",
    "b_low_shelf",
    "b_hi_shelf",
    "lag2",
    "lag3",
    "lag2_ud",
    "lag3_ud",
    "median",
    "moog_ff",
    "amplitude",
    "compander",
    "normalizer",
    "limiter",
    "freq_shift",
    "pluck",
    "pitch_shift",
    "del_tap_wr",
    "del_tap_rd",
    "free_verb",
    "spring",
    "ball",
    "tball",
]


def lpf(signal, freq=440.0) -> Ugen:
    """Second-order Butterworth lowpass: -3 dB at `freq`, -12 dB/octave."""
    return Ugen("LPF", [signal, freq])


def hpf(signal, freq=440.0) -> Ugen:
    """Second-order Butterworth highpass: -3 dB at `freq`, -12 dB/octave."""
    return Ugen("HPF", [signal, freq])


def one_pole(signal, coef=0.5) -> Ugen:
    """`y[n] = (1-|coef|)*x[n] + coef*y[n-1]` -- lowpass for a positive
    coefficient, highpass for a negative one, unity in the passband.

    The parameter is the **pole**, not a cutoff, as in sclang. Use `lag` when
    what you want is a time constant.
    """
    return Ugen("OnePole", [signal, coef])


def one_zero(signal, coef=0.5) -> Ugen:
    """`y[n] = (1-|coef|)*x[n] + coef*x[n-1]` -- the zero-only sibling of
    `one_pole`.
    """
    return Ugen("OneZero", [signal, coef])


def leak_dc(signal, coef=0.995) -> Ugen:
    """Removes DC: a zero exactly at 0 Hz with a pole just inside it. The
    default corner is low enough to leave audio untouched.
    """
    return Ugen("LeakDC", [signal, coef])


def integrator(signal, coef=0.999) -> Ugen:
    """Leaky accumulator, `y[n] = x[n] + coef*y[n-1]`. The coefficient is
    clamped just inside 1 on the server, so it always forgets eventually
    instead of running away on a DC input.
    """
    return Ugen("Integrator", [signal, coef])


def lag(signal, time=0.1) -> Ugen:
    """One-pole smoother: `signal` lagged over `time` seconds (symmetric);
    `time` 0 passes through. The same UGen the server inserts for a lagged
    control -- use it directly to smooth any signal.
    """
    return Ugen("Lag", [signal, time])


def lag_ud(signal, up=0.1, down=0.1) -> Ugen:
    """One-pole smoother with separate rise (`up`) and fall (`down`) times: the
    UGen a control with a separate downward lag inserts.
    """
    return Ugen("LagUD", [signal, up, down])


def buf_delay_n(bufnum, chan, signal, delaytime=0.2) -> Ugen:
    """`delay_n` over a channel of a pool buffer."""
    return Ugen("BufDelayN", [bufnum, chan, signal, delaytime])


def buf_delay_l(bufnum, chan, signal, delaytime=0.2) -> Ugen:
    """`delay_l` over a channel of a pool buffer."""
    return Ugen("BufDelayL", [bufnum, chan, signal, delaytime])


def buf_delay_c(bufnum, chan, signal, delaytime=0.2) -> Ugen:
    """`delay_c` over a channel of a pool buffer."""
    return Ugen("BufDelayC", [bufnum, chan, signal, delaytime])


def buf_comb_n(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`comb_n` over a channel of a pool buffer."""
    return Ugen("BufCombN", [bufnum, chan, signal, delaytime, decaytime])


def buf_comb_l(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`comb_l` over a channel of a pool buffer."""
    return Ugen("BufCombL", [bufnum, chan, signal, delaytime, decaytime])


def buf_comb_c(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`comb_c` over a channel of a pool buffer."""
    return Ugen("BufCombC", [bufnum, chan, signal, delaytime, decaytime])


def buf_allpass_n(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`allpass_n` over a channel of a pool buffer."""
    return Ugen("BufAllpassN", [bufnum, chan, signal, delaytime, decaytime])


def buf_allpass_l(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`allpass_l` over a channel of a pool buffer."""
    return Ugen("BufAllpassL", [bufnum, chan, signal, delaytime, decaytime])


def buf_allpass_c(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`allpass_c` over a channel of a pool buffer."""
    return Ugen("BufAllpassC", [bufnum, chan, signal, delaytime, decaytime])


def delay1(signal, x1=0.0) -> Ugen:
    """The input one sample late; `x1` is the sample it starts with."""
    return Ugen("Delay1", [signal, x1])


def delay2(signal, x1=0.0, x2=0.0) -> Ugen:
    """The input two samples late; `x1` and `x2` are the samples it starts with.
    """
    return Ugen("Delay2", [signal, x1, x2])


def lpz1(signal) -> Ugen:
    """Two-point average, `0.5 (x0 + x1)`: a zero at Nyquist."""
    return Ugen("LPZ1", [signal])


def hpz1(signal) -> Ugen:
    """Two-point difference, `0.5 (x0 - x1)`: a zero at DC."""
    return Ugen("HPZ1", [signal])


def lpz2(signal) -> Ugen:
    """Two-zero lowpass, `0.25 (x0 + 2 x1 + x2)`."""
    return Ugen("LPZ2", [signal])


def hpz2(signal) -> Ugen:
    """Two-zero highpass, `0.25 (x0 - 2 x1 + x2)`."""
    return Ugen("HPZ2", [signal])


def bpz2(signal) -> Ugen:
    """Two-zero bandpass, `0.5 (x0 - x2)`: zeros at DC and Nyquist."""
    return Ugen("BPZ2", [signal])


def brz2(signal) -> Ugen:
    """Two-zero band reject, `0.5 (x0 + x2)`: zeros at a quarter of the rate.
    """
    return Ugen("BRZ2", [signal])


def slope(signal) -> Ugen:
    """The signal's rate of change per second, `rate (x0 - x1)`."""
    return Ugen("Slope", [signal])


def flip(signal) -> Ugen:
    """Every other sample negated: the spectrum mirrored about a quarter of the rate.
    """
    return Ugen("Flip", [signal])


def two_zero(signal, freq=440.0, radius=0.8) -> Ugen:
    """Two zeros at `freq` with `radius`."""
    return Ugen("TwoZero", [signal, freq, radius])


def fos(signal, a0=0.0, a1=0.0, b1=0.0) -> Ugen:
    """First-order section with its coefficients as inputs: `w = x + b1 w1`, `y = a0 w + a1 w1`.
    """
    return Ugen("FOS", [signal, a0, a1, b1])


def sos(signal, a0=0.0, a1=0.0, a2=0.0, b1=0.0, b2=0.0) -> Ugen:
    """Second-order section with its coefficients as inputs: `w = x + b1 w1 + b2 w2`, `y = a0 w + a1 w1 + a2 w2`.
    """
    return Ugen("SOS", [signal, a0, a1, a2, b1, b2])


def ramp(signal, time=0.1) -> Ugen:
    """The input sampled every `time` seconds and joined by straight lines."""
    return Ugen("Ramp", [signal, time])


def slew(signal, up=1.0, down=1.0) -> Ugen:
    """The input followed at no more than `up` units per second rising and `down` falling.
    """
    return Ugen("Slew", [signal, up, down])


def two_pole(signal, freq=440.0, radius=0.8) -> Ugen:
    """Two poles at `freq` with `radius`: a resonance."""
    return Ugen("TwoPole", [signal, freq, radius])


def apf(signal, freq=440.0, radius=0.8) -> Ugen:
    """The two-pole allpass at `freq` with `radius`: phase only."""
    return Ugen("APF", [signal, freq, radius])


def ringz(signal, freq=440.0, decaytime=1.0) -> Ugen:
    """A resonator at `freq` ringing `decaytime` seconds to -60 dB."""
    return Ugen("Ringz", [signal, freq, decaytime])


def formlet(signal, freq=440.0, attacktime=1.0, decaytime=1.0) -> Ugen:
    """A resonance at `freq` that rises in `attacktime` and falls in `decaytime`.
    """
    return Ugen("Formlet", [signal, freq, attacktime, decaytime])


def mid_eq(signal, freq=440.0, rq=1.0, db=0.0) -> Ugen:
    """A boost or cut of `db` around `freq`, `rq` its bandwidth."""
    return Ugen("MidEQ", [signal, freq, rq, db])


def b_low_pass(signal, freq=1200.0, rq=1.0) -> Ugen:
    """Cookbook lowpass biquad, `rq` the reciprocal of Q."""
    return Ugen("BLowPass", [signal, freq, rq])


def b_hi_pass(signal, freq=1200.0, rq=1.0) -> Ugen:
    """Cookbook highpass biquad, `rq` the reciprocal of Q."""
    return Ugen("BHiPass", [signal, freq, rq])


def b_all_pass(signal, freq=1200.0, rq=1.0) -> Ugen:
    """Cookbook allpass biquad, `rq` the reciprocal of Q."""
    return Ugen("BAllPass", [signal, freq, rq])


def b_band_pass(signal, freq=1200.0, bw=1.0) -> Ugen:
    """Cookbook bandpass biquad, `bw` its bandwidth in octaves."""
    return Ugen("BBandPass", [signal, freq, bw])


def b_band_stop(signal, freq=1200.0, bw=1.0) -> Ugen:
    """Cookbook band-reject biquad, `bw` its bandwidth in octaves."""
    return Ugen("BBandStop", [signal, freq, bw])


def b_peak_eq(signal, freq=1200.0, rq=1.0, db=0.0) -> Ugen:
    """Cookbook peaking EQ: `db` of boost or cut at `freq`, `rq` its width."""
    return Ugen("BPeakEQ", [signal, freq, rq, db])


def b_low_shelf(signal, freq=1200.0, rs=1.0, db=0.0) -> Ugen:
    """Cookbook low shelf: `db` below `freq`, `rs` the shelf's slope (1 the steepest).
    """
    return Ugen("BLowShelf", [signal, freq, rs, db])


def b_hi_shelf(signal, freq=1200.0, rs=1.0, db=0.0) -> Ugen:
    """Cookbook high shelf: `db` above `freq`, `rs` the shelf's slope (1 the steepest).
    """
    return Ugen("BHiShelf", [signal, freq, rs, db])


def lag2(signal, time=0.1) -> Ugen:
    """Two `lag`s in a row: a smoother start than one."""
    return Ugen("Lag2", [signal, time])


def lag3(signal, time=0.1) -> Ugen:
    """Three `lag`s in a row."""
    return Ugen("Lag3", [signal, time])


def lag2_ud(signal, up=0.1, down=0.1) -> Ugen:
    """Two `lag_ud`s in a row, each with separate rise and fall times."""
    return Ugen("Lag2UD", [signal, up, down])


def lag3_ud(signal, up=0.1, down=0.1) -> Ugen:
    """Three `lag_ud`s in a row."""
    return Ugen("Lag3UD", [signal, up, down])


def median(length, signal) -> Ugen:
    """The median of the last `length` samples (1 to 31, odd for a true median):
    removes spikes shorter than half the window and keeps edges.
    """
    return Ugen("Median", [length, signal])


def moog_ff(signal, freq=100.0, gain=2.0, reset=0.0) -> Ugen:
    """The Moog ladder (zero-delay feedback): a four-pole lowpass at `freq`, its
    resonance `gain` from 0 to 4 (self-oscillating at 4); a positive `reset`
    clears it.
    """
    return Ugen("MoogFF", [signal, freq, gain, reset])


def amplitude(signal, attacktime=0.01, releasetime=0.01) -> Ugen:
    """An envelope follower: `|signal|` coming within a tenth of a rise in
    `attacktime` and of a fall in `releasetime`.
    """
    return Ugen("Amplitude", [signal, attacktime, releasetime])


def compander(
    signal,
    control=0.0,
    thresh=0.5,
    slopebelow=1.0,
    slopeabove=1.0,
    clamptime=0.01,
    relaxtime=0.1,
) -> Ugen:
    """A compressor, expander or gate: `control`'s envelope sets the gain,
    `(env / thresh)^(slope - 1)` with `slopebelow` under `thresh` and
    `slopeabove` over it, following in `clamptime` and `relaxtime`.
    """
    return Ugen(
        "Compander",
        [signal, control, thresh, slopebelow, slopeabove, clamptime, relaxtime],
    )


def normalizer(signal, level=1.0, dur=0.01) -> Ugen:
    """`signal` brought to peak at `level`, by a gain that looks `dur` ahead:
    the output is `2 dur` late.
    """
    return Ugen("Normalizer", [signal, level, dur])


def limiter(signal, level=1.0, dur=0.01) -> Ugen:
    """`signal` kept below `level` by a gain that looks `dur` ahead and moves in
    lines: the output is `2 dur` late and never passes `level`.
    """
    return Ugen("Limiter", [signal, level, dur])


def freq_shift(signal, freq=0.0, phase=0.0) -> Ugen:
    """Every frequency of `signal` moved by `freq` hertz (down for a negative
    one): single-sideband modulation. `phase` offsets the oscillator in
    cycles.
    """
    return Ugen("FreqShift", [signal, freq, phase])


def pluck(
    signal,
    trig=1.0,
    maxdelaytime=0.2,
    delaytime=0.2,
    decaytime=1.0,
    coef=0.5,
) -> Ugen:
    """A plucked string (Karplus-Strong): a rising `trig` lets one period of
    `signal` into a line `delaytime` seconds long that feeds back through a
    one-pole lowpass of coefficient `coef`, decaying by 60 dB in
    `decaytime`. `maxdelaytime` is a constant and sizes the line.
    """
    return Ugen(
        "Pluck",
        [signal, trig, maxdelaytime, delaytime, decaytime, coef],
    )


def pitch_shift(
    signal,
    window_size=0.2,
    pitch_ratio=1.0,
    pitch_dispersion=0.0,
    time_dispersion=0.0,
) -> Ugen:
    """`signal` at `pitch_ratio` times its pitch (0 to 4), by four overlapping
    grains of `window_size` seconds read at that speed. `pitch_dispersion`
    moves each grain's ratio and `time_dispersion` its start, at random, by
    up to that much. `window_size` is read at the start.
    """
    return Ugen(
        "PitchShift",
        [signal, window_size, pitch_ratio, pitch_dispersion, time_dispersion],
    )


def del_tap_wr(bufnum, signal) -> Ugen:
    """Writes `signal` round a mono buffer, cleared at its start, and outputs
    the frame each sample went to: the phase `del_tap_rd` reads behind.
    """
    return Ugen("DelTapWr", [bufnum, signal])


def del_tap_rd(bufnum, phase, deltime, interp=1.0) -> Ugen:
    """Reads the buffer a `del_tap_wr` writes, `deltime` seconds behind the
    `phase` it outputs, with no interpolation (`interp` 1), linear (2) or
    cubic (4). Any number of taps read one writer.
    """
    return Ugen("DelTapRd", [bufnum, phase, deltime, interp])


def free_verb(signal, mix=0.33, room=0.5, damp=0.5) -> Ugen:
    """Freeverb on one channel: `mix` is the wet part of the output, `room`
    the size (0 to 1) and `damp` how fast the highs die. Its lines are its
    tunings for 44.1 kHz scaled to the server's rate.
    """
    return Ugen("FreeVerb", [signal, mix, room, damp])


def spring(signal, spring=1.0, damp=0.0) -> Ugen:
    """A mass on a spring driven by the force `signal`, its velocity damped by
    `damp` a sample: the force the spring exerts. It rings at
    `sqrt(spring * sampleRate) / 2 pi`.
    """
    return Ugen("Spring", [signal, spring, damp])


def ball(signal, g=1.0, damp=0.0, friction=0.01) -> Ugen:
    """The height of a ball falling under gravity `g` onto a floor at the
    height `signal`, bouncing with its velocity damped by `damp` and held
    by `friction` once close.
    """
    return Ugen("Ball", [signal, g, damp, friction])


def tball(signal, g=10.0, damp=0.0, friction=0.01) -> Ugen:
    """`ball`'s impacts: its velocity at each bounce, 0 between."""
    return Ugen("TBall", [signal, g, damp, friction])
