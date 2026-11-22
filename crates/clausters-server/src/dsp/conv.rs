//! Partitioned convolution (`Conv`): one UGen, kernel spectra prepared
//! **off** the audio thread, flat steady-state load.
//!
//! The scheme is uniformly partitioned **overlap-save** convolution with a
//! frequency-domain delay line (FDL). The impulse response is split into `P`
//! partitions of `L` samples; each partition is zero-padded to `N = 2L` and
//! transformed **once, on the NRT thread**, by the `/buffer_gen prepare_partconv`
//! routine, which writes the spectra into an ordinary immutable pool buffer
//! (see [`layout`]). The audio thread never transforms a kernel: per hop of
//! `L` input samples it forward-transforms its own input window, multiplies-
//! accumulates against the ready-made spectra, inverse-transforms, and emits
//! the alias-free half -- `O(P)` complex MACs plus one FFT/IFFT pair.
//!
//! **Load spreading.** The `p >= 1` MAC terms of hop `n+1` only involve input
//! spectra that already exist after hop `n`, so they are accumulated across
//! the blocks *between* hops (a fair share per
//! [`UGen::process`](crate::dsp::UGen::process) call), and the hop block itself
//! does only the input FFT, the single fresh-spectrum MAC (`p = 0`) and the
//! IFFT. The steady-state cost per block is flat
//! instead of a per-hop sawtooth -- the design constraint the whole module is
//! shaped by (with the stagger, the other half of keeping spectral load
//! spikes out of the RT budget).
//!
//! **Why this is not a `PV_*`.** Fast convolution needs zero-padded,
//! rectangular segments whose hop is fixed by the partition size; the `fr`
//! chain is windowed COLA analysis-resynthesis. The two contracts are
//! incompatible (a naive spectral multiply in the chain computes *circular*
//! convolution), so `Conv` is a self-contained audio UGen -- the same split
//! scsynth makes, minus its five name variants.
//!
//! **Latency.** The first output sample leaves after one full partition of
//! input has been collected: an intrinsic latency of `L` samples, reported
//! through [`UGen::latency`](crate::dsp::UGen::latency)/`SynthNode::latency` --
//! the first consumer of the hook the auto-ordering work anticipated
//! (consumed by general server compensation on automatic bus paths).
//!
//! **Kernel swap.** The FDL holds *input* history, which is kernel-agnostic,
//! so swapping kernels never rebuilds state. When the `kernel` input moves to
//! a different (valid) buffer, the swap hop computes the tail of the old
//! kernel's output and a full fresh sum with the new one -- a one-hop cost
//! spike -- and crossfades the two over that hop's `L` samples (the
//! `Convolution2L` behavior, one frame). Replacing the *contents* of the same
//! buffer index instead is a hard switch with no crossfade: allocate the new
//! IR in a fresh buffer and move the input when the transition matters.
//!
//! **A kernel is used whole or not at all.** One prepared at another FFT size,
//! or with more partitions than the instance's FDL holds, plays silence
//! rather than a response cut to fit, and is reported once as a
//! [`ReplyKind::Fault`](crate::dsp::ReplyKind) (see [`fault`]): the live
//! server logs it and an offline render fails with it.

/// The faults a `Conv` reports (a [`ReplyKind::Fault`](crate::dsp::ReplyKind)
/// whose id is one of these): a kernel it cannot convolve with exactly, so
/// it plays silence rather than a wrong result.
pub mod fault {
    /// The kernel was prepared at another FFT size. Values: `[buffer, the
    /// kernel's partition length, this Conv's]`.
    pub const PARTITION: i32 = 1;
    /// The kernel has more partitions than this Conv holds -- convolving with
    /// the ones that fit would cut the response short. Values: `[buffer, the
    /// kernel's partitions, this Conv's capacity]`.
    pub const TOO_LONG: i32 = 2;
    /// A raw kernel (the `Convolution2` rows, `Convolution3`) is longer
    /// than the kernel the UGen holds, `framesize` samples. Values:
    /// `[buffer, the buffer's frames, the framesize]`.
    pub const RAW_TOO_LONG: i32 = 3;
}

/// The sentence for a fault of the convolution UGen `name`, built on the
/// network thread.
pub fn describe_fault(name: &str, code: i32, values: &[f32]) -> String {
    let v = |i: usize| values.get(i).copied().unwrap_or(0.0) as i64;
    match code {
        fault::PARTITION => format!(
            "{name}: kernel buffer {} was prepared with {}-sample partitions and this {name} \
             uses {}; it plays silence (prepare the kernel at the {name}'s fft_size)",
            v(0),
            v(1),
            v(2)
        ),
        fault::TOO_LONG => format!(
            "{name}: kernel buffer {} has {} partitions and this {name} holds {}; \
             it plays silence (give the {name} partitions >= {})",
            v(0),
            v(1),
            v(2),
            v(1)
        ),
        fault::RAW_TOO_LONG => format!(
            "{name}: kernel buffer {} has {} frames, more than the framesize {}; it plays \
             silence (give a framesize of at least the kernel's length, or use PartConv)",
            v(0),
            v(1),
            v(2)
        ),
        _ => format!("{name}: fault {code} {values:?}"),
    }
}

/// The names of the convolution UGens, whose faults [`describe_fault`]
/// words.
pub const NAMES: &[&str] = &[
    "Conv",
    "PartConv",
    "Convolution2",
    "Convolution2L",
    "StereoConvolution2L",
    "Convolution3",
];

/// Default maximum partition count when the def omits `partitions`: with the
/// default `fft_size` (1024, so `L = 512`), 16 partitions cover ~170 ms of IR
/// at 48 kHz. Reverb-length IRs need an explicit, larger `partitions`.
pub const DEFAULT_PARTITIONS: usize = 16;
/// Hard cap on `partitions` -- bounds the pre-allocated FDL like every other
/// boot/build-time pool (256 * 4096 floats, 4 MiB, at a 4096-point
/// transform; 32 MiB at the largest, 32768).
pub const MAX_PARTITIONS: usize = 256;

/// The prepared-kernel buffer layout written by `/buffer_gen prepare_partconv` and
/// read by `Conv`: `data[0] = L` (partition length), `data[1] = P`
/// (partition count), then `P` frames of `N = 2L` floats -- each partition
/// zero-padded to `N` and packed by
/// [`fft::rfft_into`](clausters_core::fft::rfft_into)
/// (`[dc, nyquist, re1, im1, ...]`).
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
    use std::sync::atomic::AtomicU32;

    use super::{DEFAULT_PARTITIONS, MAX_PARTITIONS, fault, layout};
    use crate::dsp::buffer::Buffer;
    use crate::dsp::fifo::SampleFifo;
    use crate::dsp::registry::UGenConfig;
    use crate::dsp::spectral::resolve_fft_size;
    use crate::dsp::{ProcessCtx, ReplyKind, ReplyMsg, UGen, at};
    use clausters_core::fft;

    /// Which scsynth name a [`Conv`] serves, which says where its inputs
    /// are and where its kernel comes from.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum ConvKind {
        /// `Conv`: `[in, kernel]`, a prepared kernel, `fft_size` static.
        Conv,
        /// `PartConv`: `[in, fftsize, irbufnum]`, a prepared kernel.
        PartConv,
        /// `Convolution2`: `[in, kernel, trigger, framesize]`, a raw
        /// kernel reloaded at each trigger, switched hard.
        Convolution2,
        /// `Convolution2L`: `[in, kernel, trigger, framesize, crossfade]`,
        /// the new kernel faded in over `crossfade` frames.
        Convolution2L,
        /// `StereoConvolution2L`: `[in, kernelL, kernelR, trigger,
        /// framesize, crossfade, chan]`, one channel of the pair.
        Stereo,
    }

    impl ConvKind {
        /// The name the kind is registered under, which its faults carry.
        pub fn name(self) -> &'static str {
            match self {
                ConvKind::Conv => "Conv",
                ConvKind::PartConv => "PartConv",
                ConvKind::Convolution2 => "Convolution2",
                ConvKind::Convolution2L => "Convolution2L",
                ConvKind::Stereo => "StereoConvolution2L",
            }
        }

        /// Whether the kernel is a buffer of samples the UGen transforms
        /// itself, rather than one `prepare_partconv` wrote.
        fn raw(self) -> bool {
            matches!(
                self,
                ConvKind::Convolution2 | ConvKind::Convolution2L | ConvKind::Stereo
            )
        }

        /// The input holding the kernel's buffer index.
        fn kernel_input(self, inputs: &[&[f32]]) -> usize {
            match self {
                ConvKind::Conv | ConvKind::Convolution2 | ConvKind::Convolution2L => 1,
                ConvKind::PartConv => 2,
                ConvKind::Stereo => {
                    let right = inputs.get(6).is_some_and(|c| at(c, 0) >= 0.5);
                    if right { 2 } else { 1 }
                }
            }
        }

        /// The trigger input of a raw kind.
        fn trigger_input(self) -> usize {
            if self == ConvKind::Stereo { 3 } else { 2 }
        }

        /// The crossfade length input (in frames) of a raw kind, if any.
        fn crossfade_input(self) -> Option<usize> {
            match self {
                ConvKind::Convolution2L => Some(4),
                ConvKind::Stereo => Some(5),
                _ => None,
            }
        }
    }

    /// Where a kernel's spectra are read: a prepared pool buffer, or the
    /// UGen's own memory, which a raw kernel is transformed into.
    #[derive(Clone, Copy)]
    enum Spectra<'a> {
        Pool(&'a [AtomicU32]),
        Own(&'a [f32]),
    }

    /// A kernel: its spectra and how many partitions they hold.
    #[derive(Clone, Copy)]
    struct Kernel<'a> {
        spectra: Spectra<'a>,
        parts: usize,
    }

    /// Which kernel a sum is for, so the accumulator spread across the
    /// blocks is known to belong to it: a pool buffer by its index, one of
    /// the two banks of own memory by its number.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Id {
        None,
        Pool(i32),
        Bank(usize),
    }

    /// `acc += spectrum * kernel` over one packed frame (DC and Nyquist are
    /// real-only slots) -- the FDL inner loop. A pool kernel comes straight
    /// out of the buffer's cells: measured, the relaxed loads cost this shape
    /// nothing (a complex MAC over a strided pair is not what the optimizer
    /// was vectorizing), so there is no copy into scratch to justify.
    #[inline]
    fn mac(acc: &mut [f32], s: &[f32], k: Spectra, p: usize, n: usize) {
        #[inline(always)]
        fn go(acc: &mut [f32], s: &[f32], n: usize, k: impl Fn(usize) -> f32) {
            acc[0] += s[0] * k(0);
            acc[1] += s[1] * k(1);
            for i in 1..n / 2 {
                let (sr, si) = (s[2 * i], s[2 * i + 1]);
                let (kr, ki) = (k(2 * i), k(2 * i + 1));
                acc[2 * i] += sr * kr - si * ki;
                acc[2 * i + 1] += sr * ki + si * kr;
            }
        }
        match k {
            Spectra::Pool(d) => {
                let base = layout::HEADER + p * n;
                go(acc, s, n, |i| Buffer::load(&d[base + i]));
            }
            Spectra::Own(d) => {
                let base = p * n;
                go(acc, s, n, |i| d[base + i]);
            }
        }
    }

    /// The overlap-save machinery shared by every row: the input window,
    /// the frequency-domain delay line and the sums over it. It knows no
    /// kernel; each call is handed the one it sums against.
    struct Fdl {
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
        /// `max_parts` packed spectra of `n` floats. `head` is the slot of
        /// the newest spectrum; older spectra sit at `(head + p) %
        /// max_parts`.
        fdl: Vec<f32>,
        head: usize,
        /// Accumulator for the upcoming hop's `p >= 1` MAC terms (spread
        /// across the blocks between hops), for kernel `acc_for`.
        acc: Vec<f32>,
        acc_for: Id,
        /// Next partition index (1-based) to accumulate into `acc`.
        pending_p: usize,
        /// Full sums for kernels the accumulator is not for.
        tmp: [Vec<f32>; 2],
        /// Time-domain scratch for the inverse transform.
        time: Vec<f32>,
        fft: fft::FftScratch,
    }

    impl Fdl {
        fn new(n: usize, max_parts: usize) -> Self {
            Self {
                part: n / 2,
                n,
                max_parts,
                inbuf: vec![0.0; n],
                write: 0,
                since_hop: 0,
                scratch: vec![0.0; n],
                fdl: vec![0.0; max_parts * n],
                head: 0,
                acc: vec![0.0; n],
                acc_for: Id::None,
                pending_p: 1,
                tmp: [vec![0.0; n], vec![0.0; n]],
                time: vec![0.0; n],
                fft: fft::FftScratch::new(n),
            }
        }

        fn reset(&mut self) {
            for line in [
                &mut self.inbuf,
                &mut self.scratch,
                &mut self.fdl,
                &mut self.acc,
            ] {
                line.fill(0.0);
            }
            self.tmp.iter_mut().for_each(|t| t.fill(0.0));
            self.time.fill(0.0);
            self.write = 0;
            self.since_hop = 0;
            self.head = 0;
            self.acc_for = Id::None;
            self.pending_p = 1;
        }

        /// Catches the pending `p >= 1` MACs of the upcoming hop up to a
        /// slice of `len` samples' fair share of the hop period, so the hop
        /// itself has none left, for kernel `id`. A kernel other than the
        /// one the accumulator was started for starts it again.
        fn spread(&mut self, id: Id, k: Kernel, len: usize) {
            if self.acc_for != id {
                self.acc.fill(0.0);
                self.pending_p = 1;
                self.acc_for = id;
            }
            if k.parts <= 1 {
                return;
            }
            let elapsed = (self.since_hop + len).min(self.part);
            let due = ((k.parts - 1) * elapsed).div_ceil(self.part);
            while self.pending_p <= due && self.pending_p < k.parts {
                let p = self.pending_p;
                let slot = (self.head + p - 1) % self.max_parts;
                let s = &self.fdl[slot * self.n..(slot + 1) * self.n];
                mac(&mut self.acc, s, k.spectra, p, self.n);
                self.pending_p += 1;
            }
        }

        /// Takes input sample `x`; true when it closes a hop.
        fn push(&mut self, x: f32) -> bool {
            self.inbuf[self.write] = x;
            self.write = (self.write + 1) % self.n;
            self.since_hop += 1;
            if self.since_hop == self.part {
                self.since_hop = 0;
                true
            } else {
                false
            }
        }

        /// Forward-transforms the newest input window into the FDL's head.
        fn transform_input(&mut self) {
            // De-circularize: `write` points at the oldest of the last n samples.
            for k in 0..self.n {
                self.scratch[k] = self.inbuf[(self.write + k) % self.n];
            }
            self.head = (self.head + self.max_parts - 1) % self.max_parts;
            let head = self.head * self.n;
            fft::rfft_with(
                &self.scratch,
                &mut self.fdl[head..head + self.n],
                &mut self.fft,
            );
        }

        /// The hop's output for kernel `id` into `out` (`part` samples): the
        /// accumulator closed when it is for `id`, a full sum otherwise
        /// (into `tmp[t]`), then the inverse transform's alias-free half.
        fn output(&mut self, id: Id, k: Kernel, t: usize, out: &mut [f32]) {
            let sum = if self.acc_for == id {
                while self.pending_p < k.parts {
                    let p = self.pending_p;
                    let slot = (self.head + p) % self.max_parts;
                    let s = &self.fdl[slot * self.n..(slot + 1) * self.n];
                    mac(&mut self.acc, s, k.spectra, p, self.n);
                    self.pending_p += 1;
                }
                let head = self.head * self.n;
                let s = &self.fdl[head..head + self.n];
                mac(&mut self.acc, s, k.spectra, 0, self.n);
                &self.acc
            } else {
                let tmp = &mut self.tmp[t];
                tmp.fill(0.0);
                for p in 0..k.parts {
                    let slot = (self.head + p) % self.max_parts;
                    let s = &self.fdl[slot * self.n..(slot + 1) * self.n];
                    mac(tmp, s, k.spectra, p, self.n);
                }
                &self.tmp[t]
            };
            fft::irfft_with(sum, &mut self.time, &mut self.fft);
            // Overlap-save: the first half of the inverse transform is
            // circular aliasing; the last `part` samples are the exact
            // linear convolution.
            out.copy_from_slice(&self.time[self.part..]);
        }

        /// Ends a hop: the accumulator starts over for kernel `id`.
        fn finish(&mut self, id: Id) {
            self.acc.fill(0.0);
            self.pending_p = 1;
            self.acc_for = id;
        }
    }

    /// A raw kernel being transformed into a bank, a partition per slice.
    #[derive(Clone, Copy)]
    struct Load {
        bank: usize,
        buf: i32,
        parts: usize,
        next: usize,
        /// The crossfade, in frames, read at the trigger.
        fade: usize,
    }

    /// A crossfade in progress from one kernel to another: `hops` frames
    /// long, `done` of them played.
    #[derive(Clone, Copy)]
    struct Fade {
        from: Id,
        hops: usize,
        done: usize,
    }

    /// Uniformly partitioned overlap-save convolver: `Conv`, `PartConv`,
    /// `Convolution2`, `Convolution2L` and one channel of
    /// `StereoConvolution2L` ([`ConvKind`] says which, and where the inputs
    /// are).
    ///
    /// A **prepared** kernel (`Conv`, `PartConv`) is a buffer `/buffer_gen
    /// prepare_partconv` wrote; moving the kernel input to another one
    /// crossfades over one partition. A **raw** kernel (the `Convolution2`
    /// rows) is a buffer of samples, channel 0, which the UGen transforms
    /// into its own memory at its first slice and again at each trigger --
    /// a partition per slice, so a reload spreads like the MACs do, and the
    /// old kernel plays until the new one is whole -- then switches to it
    /// hard (`Convolution2`) or over `crossfade` frames (the `2L` rows).
    ///
    /// Static config: the transform size (`fft_size`, or the def's
    /// `fftsize`/`framesize` constant the compiler resolved into it; the
    /// partition is half of it) and, for a prepared kernel, `partitions`,
    /// the longest kernel this instance accepts. A raw kernel holds one
    /// partition, `framesize` samples, as scsynth's does. A kernel the
    /// instance cannot use whole plays silence and is reported, never cut;
    /// an unprepared, missing or empty buffer plays silence (the input
    /// history keeps running, so a valid kernel resumes cleanly).
    pub struct Conv {
        kind: ConvKind,
        fdl: Fdl,
        /// Finalized output samples, drained one per input sample.
        fifo: SampleFifo,
        /// The kernel playing.
        cur: Id,
        fade: Option<Fade>,
        /// The two banks of own memory a raw kernel is transformed into,
        /// `max_parts * n` floats each, with the partitions each holds.
        banks: [Vec<f32>; 2],
        bank_parts: [usize; 2],
        /// Zero-padded partition handed to the kernel transform.
        kscratch: Vec<f32>,
        kfft: fft::FftScratch,
        load: Option<Load>,
        /// Whether the first slice has run (the first raw load is whole).
        started: bool,
        prev_trig: f32,
        /// One hop's output for each of the two kernels a fade mixes.
        half: [Vec<f32>; 2],
        /// A fault waiting for the synth to drain it.
        pending: Option<ReplyMsg>,
        /// The last fault reported, as `(buffer, code)`, so a kernel that
        /// stays wrong is reported once; cleared when a kernel is valid.
        reported: Option<(i32, i32)>,
    }

    /// Why a buffer is not a kernel this `Conv` can use.
    enum Unusable {
        /// Not a kernel at all (unallocated, unprepared, short, empty):
        /// silence, and nothing to report -- a kernel still being prepared
        /// looks like this for a moment.
        Absent,
        /// A kernel this instance cannot convolve with exactly: `(code, the
        /// kernel's number, this Conv's)`.
        Fault(i32, usize, usize),
    }

    impl Conv {
        pub fn new(config: &UGenConfig, kind: ConvKind) -> Self {
            let n = resolve_fft_size(config.fft_size);
            let max_parts = if kind.raw() {
                1
            } else {
                config
                    .partitions
                    .unwrap_or(DEFAULT_PARTITIONS)
                    .clamp(1, MAX_PARTITIONS)
            };
            let bank = if kind.raw() { max_parts * n } else { 0 };
            Self {
                kind,
                fdl: Fdl::new(n, max_parts),
                fifo: SampleFifo::new(4 * n),
                cur: Id::None,
                fade: None,
                banks: [vec![0.0; bank], vec![0.0; bank]],
                bank_parts: [0; 2],
                kscratch: vec![0.0; if kind.raw() { n } else { 0 }],
                kfft: fft::FftScratch::new(if kind.raw() { n } else { 64 }),
                load: None,
                started: false,
                prev_trig: 0.0,
                half: [vec![0.0; n / 2], vec![0.0; n / 2]],
                pending: None,
                reported: None,
            }
        }

        /// Queues a fault for buffer `index` the first time it is reported.
        fn report(&mut self, index: i32, code: i32, got: usize, want: usize) {
            if self.reported != Some((index, code)) {
                self.reported = Some((index, code));
                let mut msg = ReplyMsg::new(ReplyKind::Fault, code, self.kind.name());
                msg.push_value(index as f32);
                msg.push_value(got as f32);
                msg.push_value(want as f32);
                self.pending = Some(msg);
            }
        }

        /// Validates a pool buffer as a prepared kernel for this instance:
        /// matching partition length, a partition count it can hold, and
        /// enough data. A kernel longer than the FDL is refused, never cut
        /// to fit: convolving with its first partitions would be a shorter
        /// response than the one given.
        fn prepared_parts(&self, data: &[AtomicU32]) -> Result<usize, Unusable> {
            if data.len() < layout::HEADER {
                return Err(Unusable::Absent);
            }
            let part = Buffer::load(&data[0]);
            let parts = Buffer::load(&data[1]) as usize;
            if part <= 0.0 || parts == 0 || data.len() < layout::frames(2 * part as usize, parts) {
                return Err(Unusable::Absent);
            }
            if part != self.fdl.part as f32 {
                return Err(Unusable::Fault(
                    fault::PARTITION,
                    part as usize,
                    self.fdl.part,
                ));
            }
            if parts > self.fdl.max_parts {
                return Err(Unusable::Fault(fault::TOO_LONG, parts, self.fdl.max_parts));
            }
            Ok(parts)
        }

        /// Resolves prepared kernel `index`, queuing a fault the first time
        /// it is one this instance cannot use exactly.
        fn resolve<'a>(&mut self, ctx: &ProcessCtx<'a>, index: i32) -> Option<Kernel<'a>> {
            let data = pool_data(ctx, index)?;
            match self.prepared_parts(data) {
                Ok(parts) => {
                    self.reported = None;
                    Some(Kernel {
                        spectra: Spectra::Pool(data),
                        parts,
                    })
                }
                Err(Unusable::Fault(code, got, want)) => {
                    self.report(index, code, got, want);
                    None
                }
                Err(Unusable::Absent) => None,
            }
        }

        /// Starts transforming raw kernel `index` into the bank not
        /// playing; `fade` is the crossfade it will switch with. A buffer
        /// that is no kernel loads as silence; one longer than the
        /// instance holds is reported and loads as silence too.
        fn begin_load(&mut self, ctx: &ProcessCtx, index: i32, fade: usize) {
            let bank = match self.cur {
                Id::Bank(b) => 1 - b,
                _ => 0,
            };
            let frames = pool_buffer(ctx, index).map_or(0, |b| b.frames());
            let parts = frames.div_ceil(self.fdl.part);
            let parts = if parts > self.fdl.max_parts {
                self.report(
                    index,
                    fault::RAW_TOO_LONG,
                    frames,
                    self.fdl.max_parts * self.fdl.part,
                );
                0
            } else {
                if parts > 0 {
                    self.reported = None;
                }
                parts
            };
            self.load = Some(Load {
                bank,
                buf: index,
                parts,
                next: 0,
                fade,
            });
        }

        /// Transforms up to `budget` partitions of the raw kernel loading;
        /// once it is whole it becomes the kernel playing.
        fn step_load(&mut self, ctx: &ProcessCtx, budget: usize) {
            let Some(mut load) = self.load else {
                return;
            };
            let buffer = pool_buffer(ctx, load.buf);
            let (n, part) = (self.fdl.n, self.fdl.part);
            let mut left = budget;
            while load.next < load.parts && left > 0 {
                self.kscratch.fill(0.0);
                if let Some(b) = buffer {
                    let first = load.next * part;
                    for (k, slot) in self.kscratch.iter_mut().take(part).enumerate() {
                        if first + k >= b.frames() {
                            break;
                        }
                        *slot = b.sample(first + k, 0);
                    }
                }
                let p = load.next;
                let out = &mut self.banks[load.bank][p * n..(p + 1) * n];
                fft::rfft_with(&self.kscratch, out, &mut self.kfft);
                load.next += 1;
                left -= 1;
            }
            if load.next < load.parts {
                self.load = Some(load);
                return;
            }
            self.load = None;
            self.bank_parts[load.bank] = load.parts;
            let to = if load.parts == 0 {
                Id::None
            } else {
                Id::Bank(load.bank)
            };
            self.switch(to, load.fade);
        }

        /// Makes `to` the kernel playing, faded in over `hops` frames (0:
        /// at once). A fade already running ends where it is.
        fn switch(&mut self, to: Id, hops: usize) {
            let from = self.cur;
            self.cur = to;
            self.fade = (hops > 0 && from != Id::None && from != to).then_some(Fade {
                from,
                hops,
                done: 0,
            });
        }
    }

    /// Pool buffer `index`, if allocated.
    fn pool_buffer<'a>(ctx: &ProcessCtx<'a>, index: i32) -> Option<&'a Buffer> {
        if index < 0 {
            return None;
        }
        ctx.buffer(index as usize)
    }

    /// Resolves pool buffer `index` as a slice of samples, if allocated.
    fn pool_data<'a>(ctx: &ProcessCtx<'a>, index: i32) -> Option<&'a [AtomicU32]> {
        // A prepared kernel is a contiguous span of partitioned spectra, which
        // a join is not: `None` here reads as "no kernel", the same answer an
        // unallocated index gives.
        pool_buffer(ctx, index).and_then(|b| b.cells())
    }

    impl UGen for Conv {
        fn reset(&mut self) {
            self.fdl.reset();
            self.fifo.clear();
            self.cur = Id::None;
            self.fade = None;
            self.banks.iter_mut().for_each(|b| b.fill(0.0));
            self.bank_parts = [0; 2];
            self.load = None;
            self.started = false;
            self.prev_trig = 0.0;
            self.pending = None;
            self.reported = None;
        }

        fn latency(&self) -> usize {
            self.fdl.part
        }

        fn is_reply(&self) -> bool {
            true
        }

        fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
            if let Some(mut msg) = self.pending.take() {
                msg.node_id = node_id;
                sink(msg);
            }
        }

        fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
            let input = inputs[0];
            let kernel_index = at(inputs[self.kind.kernel_input(inputs)], 0).round() as i32;
            if self.kind.raw() {
                // A raw kernel loads at the first slice, whole, and again
                // at each trigger, a partition a slice.
                let trig = at(inputs[self.kind.trigger_input()], 0);
                let fade = self
                    .kind
                    .crossfade_input()
                    .map_or(0, |k| at(inputs[k], 0).max(0.0) as usize);
                if !self.started {
                    self.begin_load(ctx, kernel_index, 0);
                    self.step_load(ctx, usize::MAX);
                } else if self.prev_trig <= 0.0 && trig > 0.0 {
                    self.begin_load(ctx, kernel_index, fade);
                }
                self.prev_trig = trig;
                self.step_load(ctx, 1);
            } else {
                // A prepared kernel is the one the input names, now; moving
                // to another valid one crossfades over a partition.
                let id = Id::Pool(kernel_index);
                if self.resolve(ctx, kernel_index).is_none() {
                    if self.cur != Id::None {
                        self.switch(Id::None, 1);
                    }
                } else if self.cur != id {
                    self.switch(id, 1);
                }
            }
            self.started = true;

            // Spread the upcoming hop's MACs for the kernel playing.
            let cur = self.cur;
            let len = input.len();
            if let Some(k) = kernel_ref(&self.banks, &self.bank_parts, ctx, cur, &self.fdl) {
                self.fdl.spread(cur, k, len);
            }

            for (j, o) in output.iter_mut().enumerate() {
                // Pop before pushing: the hop fired while consuming sample
                // t = L-1 must reach the output at t = L, keeping the
                // intrinsic latency exactly the reported `part` samples.
                *o = self.fifo.pop();
                if self.fdl.push(at(input, j)) {
                    self.hop(ctx);
                }
            }
        }
    }

    /// Kernel `id` from the parts of a `Conv` it lives in, borrowed apart
    /// from its FDL so the FDL can sum against it.
    fn kernel_ref<'a>(
        banks: &'a [Vec<f32>; 2],
        bank_parts: &[usize; 2],
        ctx: &ProcessCtx<'a>,
        id: Id,
        fdl: &Fdl,
    ) -> Option<Kernel<'a>> {
        match id {
            Id::None => None,
            Id::Pool(index) => {
                let data = pool_data(ctx, index)?;
                if data.len() < layout::HEADER {
                    return None;
                }
                let part = Buffer::load(&data[0]);
                let parts = Buffer::load(&data[1]) as usize;
                let fits = part == fdl.part as f32
                    && parts > 0
                    && parts <= fdl.max_parts
                    && data.len() >= layout::frames(fdl.n, parts);
                fits.then_some(Kernel {
                    spectra: Spectra::Pool(data),
                    parts,
                })
            }
            Id::Bank(b) => (bank_parts[b] > 0).then(|| Kernel {
                spectra: Spectra::Own(&banks[b]),
                parts: bank_parts[b],
            }),
        }
    }

    impl Conv {
        /// One hop: the newest input window into the FDL, the output of the
        /// kernel playing -- mixed with the one it is fading from, while a
        /// fade runs -- and the accumulator started over for the next.
        fn hop(&mut self, ctx: &ProcessCtx) {
            self.fdl.transform_input();
            let part = self.fdl.part;
            let cur = self.cur;
            let Conv {
                fdl,
                banks,
                bank_parts,
                half,
                ..
            } = self;
            let [new_half, old_half] = half;
            let new = kernel_ref(banks, bank_parts, ctx, cur, fdl);
            match new {
                Some(k) => fdl.output(cur, k, 0, new_half),
                None => new_half.fill(0.0),
            }
            if let Some(mut fade) = self.fade {
                match kernel_ref(banks, bank_parts, ctx, fade.from, fdl) {
                    Some(k) => fdl.output(fade.from, k, 1, old_half),
                    None => old_half.fill(0.0),
                }
                let total = (fade.hops * part) as f32;
                for k in 0..part {
                    let t = ((fade.done * part + k) as f32 + 0.5) / total;
                    let v = old_half[k] * (1.0 - t) + new_half[k] * t;
                    self.fifo.push(v);
                }
                fade.done += 1;
                self.fade = (fade.done < fade.hops).then_some(fade);
            } else {
                for &v in new_half.iter() {
                    self.fifo.push(v);
                }
            }
            self.fdl.finish(cur);
        }
    }

    /// Direct convolution in the time domain (`Convolution3`): inputs
    /// `[in, kernel, trigger, framesize]`, each output sample the sum of the
    /// last `framesize` inputs weighted by the kernel -- no latency, and a
    /// cost per sample of the kernel's length, for short kernels. The
    /// kernel is channel 0 of a buffer of samples, read at the first slice
    /// and again at each trigger; one longer than `framesize` is reported
    /// and plays silence, one shorter is padded with zeros.
    ///
    /// Departure from scsynth's code: scsynth's output ring is added into
    /// and never cleared, so every sample it plays carries every earlier
    /// one; this is the convolution.
    pub struct DirectConv {
        kernel: Vec<f32>,
        /// The last `framesize` inputs, newest at `write - 1`.
        history: Vec<f32>,
        write: usize,
        started: bool,
        prev_trig: f32,
        pending: Option<ReplyMsg>,
        reported: Option<i32>,
    }

    impl DirectConv {
        pub fn new(config: &UGenConfig) -> Self {
            let size = config.constant(3, 2048.0).max(1.0) as usize;
            Self {
                kernel: vec![0.0; size],
                history: vec![0.0; size],
                write: 0,
                started: false,
                prev_trig: 0.0,
                pending: None,
                reported: None,
            }
        }

        fn load(&mut self, ctx: &ProcessCtx, index: i32) {
            self.kernel.fill(0.0);
            let Some(b) = pool_buffer(ctx, index) else {
                return;
            };
            let size = self.kernel.len();
            if b.frames() > size {
                if self.reported != Some(index) {
                    self.reported = Some(index);
                    let mut msg =
                        ReplyMsg::new(ReplyKind::Fault, fault::RAW_TOO_LONG, "Convolution3");
                    msg.push_value(index as f32);
                    msg.push_value(b.frames() as f32);
                    msg.push_value(size as f32);
                    self.pending = Some(msg);
                }
                return;
            }
            self.reported = None;
            for (k, v) in self.kernel.iter_mut().take(b.frames()).enumerate() {
                *v = b.sample(k, 0);
            }
        }
    }

    impl UGen for DirectConv {
        fn reset(&mut self) {
            self.kernel.fill(0.0);
            self.history.fill(0.0);
            self.write = 0;
            self.started = false;
            self.prev_trig = 0.0;
            self.pending = None;
            self.reported = None;
        }

        fn is_reply(&self) -> bool {
            true
        }

        fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
            if let Some(mut msg) = self.pending.take() {
                msg.node_id = node_id;
                sink(msg);
            }
        }

        fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
            let trig = at(inputs[2], 0);
            let index = at(inputs[1], 0).round() as i32;
            if !self.started || (self.prev_trig <= 0.0 && trig > 0.0) {
                self.load(ctx, index);
            }
            self.started = true;
            self.prev_trig = trig;
            let size = self.history.len();
            for (j, o) in output.iter_mut().enumerate() {
                self.history[self.write] = at(inputs[0], j);
                self.write = (self.write + 1) % size;
                // kernel[i] weighs the input i samples ago.
                let mut sum = 0.0;
                let newest = self.write + size - 1;
                for (i, &k) in self.kernel.iter().enumerate() {
                    sum += k * self.history[(newest - i) % size];
                }
                *o = sum;
            }
        }
    }

    /// Convolution of two signals (`Convolution`): inputs `[in, kernel,
    /// framesize]`, both audio. Each `framesize` samples of the input are
    /// convolved with the same `framesize` samples of the kernel signal --
    /// a kernel that changes as it plays, which is what makes it a
    /// cross-synthesis -- and the results overlap-added. Latency
    /// `framesize` samples.
    pub struct SignalConv {
        size: usize,
        a: Vec<f32>,
        b: Vec<f32>,
        pos: usize,
        fa: Vec<f32>,
        fb: Vec<f32>,
        /// The zero-padded segments handed to the transform.
        pad: Vec<f32>,
        time: Vec<f32>,
        tail: Vec<f32>,
        fifo: SampleFifo,
        fft: fft::FftScratch,
    }

    impl SignalConv {
        pub fn new(config: &UGenConfig) -> Self {
            let n = resolve_fft_size(config.fft_size);
            let size = n / 2;
            Self {
                size,
                a: vec![0.0; size],
                b: vec![0.0; size],
                pos: 0,
                fa: vec![0.0; n],
                fb: vec![0.0; n],
                pad: vec![0.0; n],
                time: vec![0.0; n],
                tail: vec![0.0; size],
                fifo: SampleFifo::new(4 * n),
                fft: fft::FftScratch::new(n),
            }
        }

        fn frame(&mut self) {
            let size = self.size;
            self.pad.fill(0.0);
            self.pad[..size].copy_from_slice(&self.a);
            fft::rfft_with(&self.pad, &mut self.fa, &mut self.fft);
            self.pad[..size].copy_from_slice(&self.b);
            fft::rfft_with(&self.pad, &mut self.fb, &mut self.fft);
            let (fa, fb) = (&mut self.fa, &self.fb);
            fa[0] *= fb[0];
            fa[1] *= fb[1];
            for i in 1..size {
                let (ar, ai) = (fa[2 * i], fa[2 * i + 1]);
                let (br, bi) = (fb[2 * i], fb[2 * i + 1]);
                fa[2 * i] = ar * br - ai * bi;
                fa[2 * i + 1] = ar * bi + ai * br;
            }
            fft::irfft_with(&self.fa, &mut self.time, &mut self.fft);
            for k in 0..size {
                self.fifo.push(self.time[k] + self.tail[k]);
            }
            self.tail.copy_from_slice(&self.time[size..]);
        }
    }

    impl UGen for SignalConv {
        fn reset(&mut self) {
            for v in [&mut self.a, &mut self.b, &mut self.tail, &mut self.time] {
                v.fill(0.0);
            }
            self.pos = 0;
            self.fifo.clear();
        }

        fn latency(&self) -> usize {
            self.size
        }

        fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
            for (j, o) in output.iter_mut().enumerate() {
                *o = self.fifo.pop();
                self.a[self.pos] = at(inputs[0], j);
                self.b[self.pos] = at(inputs[1], j);
                self.pos += 1;
                if self.pos == self.size {
                    self.pos = 0;
                    self.frame();
                }
            }
        }
    }
}

#[cfg(feature = "synth")]
pub use ugen::{Conv, ConvKind, DirectConv, SignalConv};
