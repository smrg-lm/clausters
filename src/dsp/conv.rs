//! Partitioned convolution (`Conv`, M28): one UGen, kernel spectra prepared
//! **off** the audio thread, flat steady-state load.
//!
//! The scheme is uniformly partitioned **overlap-save** convolution with a
//! frequency-domain delay line (FDL). The impulse response is split into `P`
//! partitions of `L` samples; each partition is zero-padded to `N = 2L` and
//! transformed **once, on the NRT thread**, by the `/b_gen prepare_partconv`
//! routine, which writes the spectra into an ordinary immutable pool buffer
//! (see [`layout`]). The audio thread never transforms a kernel: per hop of
//! `L` input samples it forward-transforms its own input window, multiplies-
//! accumulates against the ready-made spectra, inverse-transforms, and emits
//! the alias-free half — `O(P)` complex MACs plus one FFT/IFFT pair.
//!
//! **Load spreading.** The `p ≥ 1` MAC terms of hop `n+1` only involve input
//! spectra that already exist after hop `n`, so they are accumulated across
//! the blocks *between* hops (a fair share per
//! [`UGen::process`](crate::dsp::UGen::process) call), and the hop block itself
//! does only the input FFT, the single fresh-spectrum MAC (`p = 0`) and the
//! IFFT. The steady-state cost per block is flat
//! instead of a per-hop sawtooth — the design constraint the whole module is
//! shaped by (with the S11 stagger, the other half of keeping spectral load
//! spikes out of the RT budget).
//!
//! **Why this is not a `PV_*`.** Fast convolution needs zero-padded,
//! rectangular segments whose hop is fixed by the partition size; the `fr`
//! chain is windowed COLA analysis-resynthesis. The two contracts are
//! incompatible (a naive spectral multiply in the chain computes *circular*
//! convolution), so `Conv` is a self-contained audio UGen — the same split
//! scsynth makes, minus its five name variants.
//!
//! **Latency.** The first output sample leaves after one full partition of
//! input has been collected: an intrinsic latency of `L` samples, reported
//! through [`UGen::latency`](crate::dsp::UGen::latency)/`SynthNode::latency` —
//! the first consumer of the hook the auto-ordering work anticipated
//! (compensation itself is deferred; see `docs/model-vs-daw.md`).
//!
//! **Kernel swap.** The FDL holds *input* history, which is kernel-agnostic,
//! so swapping kernels never rebuilds state. When the `kernel` input moves to
//! a different (valid) buffer, the swap hop computes the tail of the old
//! kernel's output and a full fresh sum with the new one — a one-hop cost
//! spike — and crossfades the two over that hop's `L` samples (the
//! `Convolution2L` behavior, one frame). Replacing the *contents* of the same
//! buffer index instead is a hard switch with no crossfade: allocate the new
//! IR in a fresh buffer and move the input when the transition matters.

/// Default maximum partition count when the def omits `partitions`: with the
/// default `fft_size` (1024, so `L = 512`), 16 partitions cover ~170 ms of IR
/// at 48 kHz. Reverb-length IRs need an explicit, larger `partitions`.
pub const DEFAULT_PARTITIONS: usize = 16;
/// Hard cap on `partitions` — bounds the pre-allocated FDL like every other
/// boot/build-time pool (256 × 4096 floats ≈ 4 MiB at the largest window).
pub const MAX_PARTITIONS: usize = 256;

/// The prepared-kernel buffer layout written by `/b_gen prepare_partconv` and
/// read by [`Conv`]: `data[0] = L` (partition length), `data[1] = P`
/// (partition count), then `P` frames of `N = 2L` floats — each partition
/// zero-padded to `N` and packed by
/// [`fft::rfft_into`](clausters_core::fft::rfft_into)
/// (`[dc, nyquist, re₁, im₁, …]`).
pub mod layout {
    /// Header length in samples (`[L, P]`).
    pub const HEADER: usize = 2;

    /// Frames a target buffer needs for `parts` partitions of window `n`.
    pub fn frames(n: usize, parts: usize) -> usize {
        HEADER + parts * n
    }
}

#[cfg(feature = "synth")]
mod ugen {
    use super::layout;
    use super::{DEFAULT_PARTITIONS, MAX_PARTITIONS};
    use crate::dsp::registry::UGenConfig;
    use crate::dsp::spectral::resolve_fft_size;
    use crate::dsp::{ProcessCtx, UGen, at};
    use clausters_core::fft;

    /// Uniformly partitioned overlap-save convolver. Inputs: `[in, kernel]` — the
    /// audio signal and the buffer index of a **prepared** kernel (`/b_gen
    /// prepare_partconv`). Static config: `fft_size` (the transform size `N`; the
    /// partition is `L = N/2`) and `partitions` (the FDL capacity — the longest
    /// kernel this instance accepts). A kernel whose own `L` differs from the
    /// instance's, or an unprepared/missing buffer, plays silence (the input
    /// history keeps running, so a valid kernel resumes cleanly).
    pub struct Conv {
        /// Partition length `L` (the hop, and the intrinsic latency).
        part: usize,
        /// Transform size `N = 2L`.
        n: usize,
        /// FDL capacity in partitions.
        max_parts: usize,
        /// Sliding input, a ring of the last `n` samples.
        inbuf: Vec<f32>,
        write: usize,
        /// Samples since the last hop (`0..part`).
        since_hop: usize,
        /// De-circularized input window handed to the forward transform.
        scratch: Vec<f32>,
        /// The frequency-domain delay line: `max_parts` packed spectra of `n`
        /// floats. `fdl_head` is the slot of the newest spectrum; older spectra
        /// sit at `(fdl_head + p) % max_parts`.
        fdl: Vec<f32>,
        fdl_head: usize,
        /// Accumulator for the upcoming hop's `p ≥ 1` MAC terms (spread across
        /// the blocks between hops).
        acc: Vec<f32>,
        /// Next partition index (1-based) to accumulate into `acc`.
        pending_p: usize,
        /// Full-sum scratch for the swap hop's fresh-kernel output.
        tmp: Vec<f32>,
        /// Time-domain scratch for the inverse transform.
        time: Vec<f32>,
        /// Old-kernel output held during a swap hop's crossfade.
        fade: Vec<f32>,
        /// Finalized output samples, a ring drained one per input sample.
        fifo: Vec<f32>,
        fifo_head: usize,
        fifo_tail: usize,
        fifo_len: usize,
        /// The kernel buffer index in use (rounded input 1); `-1` before any.
        kernel_buf: i32,
    }

    impl Conv {
        pub fn new(config: &UGenConfig) -> Self {
            let n = resolve_fft_size(config.fft_size);
            let part = n / 2;
            let max_parts = config
                .partitions
                .unwrap_or(DEFAULT_PARTITIONS)
                .clamp(1, MAX_PARTITIONS);
            Self {
                part,
                n,
                max_parts,
                inbuf: vec![0.0; n],
                write: 0,
                since_hop: 0,
                scratch: vec![0.0; n],
                fdl: vec![0.0; max_parts * n],
                fdl_head: 0,
                acc: vec![0.0; n],
                pending_p: 1,
                tmp: vec![0.0; n],
                time: vec![0.0; n],
                fade: vec![0.0; part],
                fifo: vec![0.0; 4 * n],
                fifo_head: 0,
                fifo_tail: 0,
                fifo_len: 0,
                kernel_buf: -1,
            }
        }

        #[inline]
        fn fifo_push(&mut self, v: f32) {
            if self.fifo_len < self.fifo.len() {
                self.fifo[self.fifo_tail] = v;
                self.fifo_tail = (self.fifo_tail + 1) % self.fifo.len();
                self.fifo_len += 1;
            }
        }

        #[inline]
        fn fifo_pop(&mut self) -> f32 {
            if self.fifo_len == 0 {
                return 0.0;
            }
            let v = self.fifo[self.fifo_head];
            self.fifo_head = (self.fifo_head + 1) % self.fifo.len();
            self.fifo_len -= 1;
            v
        }

        /// `acc += spectrum · kernel` over one packed frame (DC and Nyquist are
        /// real-only slots) — the FDL inner loop.
        #[inline]
        fn mac(acc: &mut [f32], s: &[f32], k: &[f32], n: usize) {
            acc[0] += s[0] * k[0];
            acc[1] += s[1] * k[1];
            for i in 1..n / 2 {
                let (sr, si) = (s[2 * i], s[2 * i + 1]);
                let (kr, ki) = (k[2 * i], k[2 * i + 1]);
                acc[2 * i] += sr * kr - si * ki;
                acc[2 * i + 1] += sr * ki + si * kr;
            }
        }

        /// Full sum for the newest FDL state against `kernel` (all `parts`
        /// terms), used on a swap hop for the incoming kernel. `dst` is zeroed.
        fn full_sum(&mut self, kernel: &[f32], parts: usize) {
            self.tmp.fill(0.0);
            for p in 0..parts {
                let slot = (self.fdl_head + p) % self.max_parts;
                let s = &self.fdl[slot * self.n..(slot + 1) * self.n];
                let k = &kernel[layout::HEADER + p * self.n..layout::HEADER + (p + 1) * self.n];
                Self::mac(&mut self.tmp, s, k, self.n);
            }
        }

        /// Validates a pool buffer as a prepared kernel for this instance:
        /// matching partition length, a sane partition count, and enough data.
        /// Returns the partition count in use (clamped to the FDL capacity).
        fn kernel_parts(&self, data: &[f32]) -> Option<usize> {
            if data.len() < layout::HEADER || data[0] != self.part as f32 {
                return None;
            }
            let parts = data[1] as usize;
            if parts == 0 || data.len() < layout::frames(self.n, parts) {
                return None;
            }
            Some(parts.min(self.max_parts))
        }
    }

    /// Resolves pool buffer `index` as a slice of samples, if allocated.
    fn pool_data<'a>(ctx: &ProcessCtx<'a>, index: i32) -> Option<&'a [f32]> {
        if index < 0 {
            return None;
        }
        ctx.buffers
            .get(index as usize)
            .and_then(|b| b.as_deref())
            .map(|b| b.data())
    }

    impl UGen for Conv {
        fn latency(&self) -> usize {
            self.part
        }

        fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
            let input = inputs[0];
            let kernel_buf = at(inputs[1], 0).round() as i32;
            // Resolve the requested kernel once per slice; `None` plays silence
            // but keeps the input history running. On a pending swap, also
            // resolve the outgoing kernel so its output can be crossfaded out.
            let kernel = pool_data(ctx, kernel_buf)
                .and_then(|d| self.kernel_parts(d).map(|parts| (d, parts)));
            let old_kernel = if kernel_buf != self.kernel_buf {
                pool_data(ctx, self.kernel_buf)
                    .and_then(|d| self.kernel_parts(d).map(|parts| (d, parts)))
            } else {
                None
            };

            // Spread work: catch the pending `p >= 1` MACs of the upcoming hop up
            // to this slice's fair share of the hop period, so the hop itself has
            // none left. The accumulation always uses the kernel `acc` was
            // started with (the one at `self.kernel_buf`) — after a swap request
            // that is `old_kernel` until the swap hop lands.
            let spread = if old_kernel.is_some() {
                old_kernel
            } else if kernel_buf == self.kernel_buf {
                kernel
            } else {
                None // swapping away from an invalid/vanished kernel
            };
            if let Some((data, parts)) = spread
                && parts > 1
            {
                let elapsed = (self.since_hop + input.len()).min(self.part);
                let due = ((parts - 1) * elapsed).div_ceil(self.part);
                while self.pending_p <= due {
                    let p = self.pending_p;
                    let slot = (self.fdl_head + p - 1) % self.max_parts;
                    let (s0, s1) = (slot * self.n, (slot + 1) * self.n);
                    let (k0, k1) = (
                        layout::HEADER + p * self.n,
                        layout::HEADER + (p + 1) * self.n,
                    );
                    // `acc` and `fdl` are distinct fields; reborrow both at once.
                    let Conv { acc, fdl, .. } = self;
                    Self::mac(acc, &fdl[s0..s1], &data[k0..k1], self.n);
                    self.pending_p += 1;
                }
            }

            for (j, o) in output.iter_mut().enumerate() {
                // Pop before pushing: the hop fired while consuming sample
                // t = L-1 must reach the output at t = L, keeping the
                // intrinsic latency exactly the reported `part` samples.
                *o = self.fifo_pop();
                self.inbuf[self.write] = at(input, j);
                self.write = (self.write + 1) % self.n;
                self.since_hop += 1;
                if self.since_hop == self.part {
                    self.hop(kernel, old_kernel, kernel_buf);
                    self.since_hop = 0;
                }
            }
        }
    }

    impl Conv {
        /// Closes the sum `acc` was building (its remaining `p >= 1` terms plus
        /// the fresh spectrum's `p = 0` term) against `data`, leaving the result
        /// in `acc`. Called with the post-shift `fdl_head` (the fresh spectrum),
        /// so a pending term `p` pairs with FDL slot `head + p`.
        fn close_sum(&mut self, data: &[f32], parts: usize) {
            while self.pending_p < parts {
                let p = self.pending_p;
                let slot = (self.fdl_head + p) % self.max_parts;
                let (s0, s1) = (slot * self.n, (slot + 1) * self.n);
                let (k0, k1) = (
                    layout::HEADER + p * self.n,
                    layout::HEADER + (p + 1) * self.n,
                );
                let Conv { acc, fdl, .. } = self;
                Self::mac(acc, &fdl[s0..s1], &data[k0..k1], self.n);
                self.pending_p += 1;
            }
            let head = self.fdl_head * self.n;
            let (k0, k1) = (layout::HEADER, layout::HEADER + self.n);
            let Conv { acc, fdl, .. } = self;
            Self::mac(acc, &fdl[head..head + self.n], &data[k0..k1], self.n);
        }

        /// One hop: forward-transform the newest input window into the FDL, close
        /// the output sum, inverse-transform and emit the alias-free half. Runs
        /// inside the sample loop (allocation-free) so the hop boundary is
        /// sample-exact regardless of block splits.
        fn hop(
            &mut self,
            kernel: Option<(&[f32], usize)>,
            old_kernel: Option<(&[f32], usize)>,
            kernel_buf: i32,
        ) {
            // De-circularize: `write` points at the oldest of the last n samples.
            for k in 0..self.n {
                self.scratch[k] = self.inbuf[(self.write + k) % self.n];
            }
            self.fdl_head = (self.fdl_head + self.max_parts - 1) % self.max_parts;
            {
                let Conv { fdl, scratch, .. } = self;
                let head = self.fdl_head * self.n;
                fft::rfft_into(scratch, &mut fdl[head..head + self.n]);
            }

            let swap = self.kernel_buf != kernel_buf;
            let mut faded = false;
            if let Some((data, parts)) = old_kernel {
                // Outgoing kernel's output for this hop, kept for the crossfade.
                self.close_sum(data, parts);
                fft::irfft_into(&self.acc, &mut self.time);
                self.fade.copy_from_slice(&self.time[self.part..]);
                faded = true;
            }

            match kernel {
                Some((data, parts)) => {
                    if swap {
                        // Swap hop: a full fresh sum with the incoming kernel —
                        // the one deliberate cost spike.
                        self.full_sum(data, parts);
                        std::mem::swap(&mut self.acc, &mut self.tmp);
                    } else {
                        self.close_sum(data, parts);
                    }
                    fft::irfft_into(&self.acc, &mut self.time);
                    // Overlap-save: the first half of the inverse transform is
                    // circular aliasing; the last `part` samples are the exact
                    // linear convolution.
                    if faded {
                        for k in 0..self.part {
                            let t = (k as f32 + 0.5) / self.part as f32;
                            let v = self.fade[k] * (1.0 - t) + self.time[self.part + k] * t;
                            self.fifo_push(v);
                        }
                    } else {
                        for k in 0..self.part {
                            self.fifo_push(self.time[self.part + k]);
                        }
                    }
                }
                None => {
                    // No (valid) incoming kernel: fade the old one out if there
                    // is one, silence otherwise.
                    if faded {
                        for k in 0..self.part {
                            let t = (k as f32 + 0.5) / self.part as f32;
                            self.fifo_push(self.fade[k] * (1.0 - t));
                        }
                    } else {
                        for _ in 0..self.part {
                            self.fifo_push(0.0);
                        }
                    }
                }
            }
            self.kernel_buf = kernel_buf;
            self.acc.fill(0.0);
            self.pending_p = 1;
        }
    }
}

#[cfg(feature = "synth")]
pub use ugen::Conv;
