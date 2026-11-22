//! Scalar / initial-rate (`ir`) UGens: computed once at synth init and
//! held for the node's life -- and the information UGens beside them, the
//! engine's facts a def reads, which at control rate follow the facts that
//! move. They are the concrete proof of the `ir` init
//! pass in [`crate::synthdef::instance`]: the synth runs each `ir` UGen on the
//! very first block and then never again, so a value that would *differ* if
//! recomputed (like `Rand`, [`crate::dsp::random::Once`]) stays frozen -- that freeze is the whole point of
//! the rate.

use crate::dsp::{BLOCK_SIZE, ProcessCtx, UGen};

/// `SampleRate.ir`: the engine's sample rate in Hz. No inputs. Idempotent
/// (the same every block), so it is the gentle textbook `ir` example.
///
/// This is one of the two places that read the **engine's** rate rather than
/// the running UGen's: the answer is a hardware fact, so `SampleRate.kr`
/// reports the audio rate, not the control rate it is itself running at.
pub struct SampleRate;

impl UGen for SampleRate {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(ctx.full_sample_rate);
    }
}

/// Which engine fact an [`Info`] reports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InfoKind {
    /// `BlockSize`: the samples in a control period.
    BlockSize,
    /// `ControlRate`: control periods a second.
    ControlRate,
    /// `ControlDur`: seconds in a control period.
    ControlDur,
    /// `SampleDur`: seconds in a sample.
    SampleDur,
    /// `RadiansPerSample`: `2 pi` over the sample rate.
    RadiansPerSample,
    /// `SubsampleOffset`: where within its first sample the bundle that
    /// started the synth fell, `[0, 1)`.
    SubsampleOffset(f32),
    /// `NumAudioBuses`: the audio buses.
    NumAudioBuses,
    /// `NumControlBuses`: the control buses.
    NumControlBuses,
    /// `NumInputBuses`: the hardware inputs.
    NumInputBuses,
    /// `NumOutputBuses`: the hardware outputs.
    NumOutputBuses,
    /// `NumBuffers`: the buffer pool's slots.
    NumBuffers,
    /// `NodeID`: the id of the node the UGen runs in.
    NodeId,
    /// `NumRunningSynths`: the synths in the tree.
    NumRunningSynths,
}

/// The information UGens: one engine fact each, no inputs. The rates are the
/// engine's, whatever the UGen's own, as `SampleRate`'s is. At control rate
/// `NumRunningSynths` follows the tree.
pub struct Info(pub InfoKind);

impl UGen for Info {
    fn reset(&mut self) {}

    fn process(&mut self, ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.full_sample_rate as f64;
        let block = BLOCK_SIZE as f64;
        let v = match self.0 {
            InfoKind::BlockSize => block,
            InfoKind::ControlRate => sr / block,
            InfoKind::ControlDur => block / sr,
            InfoKind::SampleDur => 1.0 / sr,
            InfoKind::RadiansPerSample => std::f64::consts::TAU / sr,
            InfoKind::SubsampleOffset(x) => x as f64,
            InfoKind::NumAudioBuses => ctx.buses.audio_count() as f64,
            InfoKind::NumControlBuses => ctx.buses.control.len() as f64,
            InfoKind::NumInputBuses => ctx.server.input_channels as f64,
            InfoKind::NumOutputBuses => ctx.server.output_channels as f64,
            InfoKind::NumBuffers => ctx.buffers.len() as f64,
            InfoKind::NodeId => ctx.server.node_id as f64,
            InfoKind::NumRunningSynths => ctx.server.running_synths as f64,
        };
        output.fill(v as f32);
    }
}
