/**
 * The smoothing-window types an `fft` (and `ifft`) carries, and the `"window"`
 * a `uCmd` on a running FFT sets (mirrors `clausters._native.Window`, which
 * `clausters.defs` exports).
 *
 * The values are `clausters_core::window::Window`'s, so a number the server
 * reads names the same window in every client.
 *
 * @module
 */

/** The smoothing-window types, by name. */
export const Window = {
    RECTANGULAR: -1,
    HANN: 0,
    SINE: 1,
    WELCH: 2,
    HAMMING: 3,
    BLACKMAN: 4,
} as const;

/** One of the {@link Window} values. */
export type Window = (typeof Window)[keyof typeof Window];
