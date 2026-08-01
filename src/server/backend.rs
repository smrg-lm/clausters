//! Real-time audio backend on top of cpal.
//!
//! cpal delivers interleaved buffers of variable size, not necessarily a
//! multiple of [`BLOCK_SIZE`]; `BlockAdapter` slices them up by requesting
//! blocks from the engine and keeping the leftover across callbacks.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use rtrb::Producer;

use crate::dsp::Limits;
use crate::server::engine::{BLOCK_SIZE, Engine, EngineHandle, engine_pair_full};

/// How many blocks of input the ring between the cpal input callback and the
/// engine can hold before overflowing (the callback drops the excess; the
/// engine reads silence on an underrun). A handful of blocks absorbs the
/// buffer-size mismatch between the two streams.
const INPUT_RING_BLOCKS: usize = 8;

pub struct AudioBackend {
    pub sample_rate: f32,
    pub channels: usize,
    /// Live hardware input channels actually opened (0 if none / unavailable).
    pub input_channels: usize,
    // The streams stop when dropped: the backend must be kept alive.
    _stream: cpal::Stream,
    _input_stream: Option<cpal::Stream>,
}

struct BlockAdapter {
    engine: Engine,
    buf: Vec<f32>,
    pos: usize,
    /// One-shot pin + scheduling diagnostic of the callback thread, run from
    /// the callback itself (`rtprio` builds only; see `server::rt`).
    #[cfg(feature = "rtprio")]
    rt_setup: crate::server::rt::RtSetup,
}

impl BlockAdapter {
    fn new(engine: Engine) -> Self {
        let len = BLOCK_SIZE * engine.channels();
        Self {
            engine,
            buf: vec![0.0; len],
            pos: len, // forces a process_block on the first sample
            #[cfg(feature = "rtprio")]
            rt_setup: crate::server::rt::RtSetup::new(),
        }
    }

    #[inline]
    fn next_sample(&mut self) -> f32 {
        if self.pos == self.buf.len() {
            self.engine.process_block(&mut self.buf);
            self.pos = 0;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        s
    }
}

/// Opens the default output device and starts the engine on its callback,
/// with `workers` M13 DSP threads for `/group_parallel` groups (0 = sequential),
/// the boot-time pool `limits`, and, optionally, an M14 IPC segment (shared
/// clock + control buses).
///
/// `outputs` requests a specific hardware output-channel count (scsynth `-o`),
/// `None` following the device default; `inputs` (scsynth `-i`, 0 = none)
/// additionally opens the default **input** device, so `In`/`In.ar` read live
/// samples from audio buses `outputs..outputs + inputs`. Both channel counts
/// are negotiated with the host and degrade gracefully (a host that fixes the
/// count keeps its own; an unavailable input device leaves the server
/// output-only). Returns the handle the network thread uses to talk to the
/// engine.
#[allow(clippy::too_many_arguments)]
pub fn start(
    workers: usize,
    ipc: Option<std::sync::Arc<crate::server::ipc::Segment>>,
    requested_sample_rate: Option<u32>,
    audio_buses: usize,
    control_buses: usize,
    limits: Limits,
    outputs: Option<usize>,
    inputs: usize,
) -> Result<(AudioBackend, EngineHandle), Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no output device available")?;
    let default = device.default_output_config()?;
    let format = default.sample_format();
    let default_channels = default.channels();
    let device_rate = default.sample_rate();

    // Requested output channels (scsynth `-o`), falling back to the device
    // default when a host fixes the count. Try the requested count first, then
    // the default, exactly like the rate fallback below.
    let want_channels = outputs
        .map(|o| o.max(1).min(u16::MAX as usize) as u16)
        .unwrap_or(default_channels);
    let channel_opts: &[u16] = if want_channels != default_channels {
        &[want_channels, default_channels]
    } else {
        std::slice::from_ref(&want_channels)
    };

    // Impose the requested rate by building the stream at it directly: PipeWire
    // honors arbitrary per-application rates (resampling to the graph rate
    // transparently), so we do not gate on `supported_output_configs`, which
    // under-reports there. Hosts that reject the rate (CoreAudio, WASAPI, plain
    // ALSA) make `build_*_stream` fail, and we fall back to the device's own
    // rate — the gap then shows up as `nominal != actual` in `/server_status.reply`.
    let rates = match requested_sample_rate {
        Some(hz) if hz != device_rate => [Some(hz), Some(device_rate)],
        _ => [Some(device_rate), None],
    };
    let mut last_err: Option<cpal::Error> = None;
    for channels in channel_opts.iter().copied() {
        for rate in rates.into_iter().flatten() {
            let cfg = cpal::StreamConfig {
                channels,
                sample_rate: rate,
                buffer_size: cpal::BufferSize::Default,
            };
            // The input buses live above the outputs, so the audio-bus space
            // must cover both; the engine clamps to the 128 ceiling.
            let audio_buses = audio_buses.max(channels as usize + inputs);
            let (mut engine, mut handle) = engine_pair_full(
                rate as f32,
                channels as usize,
                workers,
                ipc.clone(),
                audio_buses,
                control_buses,
                limits,
            );
            // Wire the input ring before the engine is moved into the adapter;
            // the producer is kept to feed the input stream once the output
            // stream is committed (a failed attempt just drops it).
            let input_producer = (inputs > 0)
                .then(|| engine.input_ring(inputs, inputs * BLOCK_SIZE * INPUT_RING_BLOCKS));
            let adapter = BlockAdapter::new(engine);
            let built = match format {
                cpal::SampleFormat::F32 => build_stream::<f32>(&device, cfg, adapter),
                cpal::SampleFormat::I16 => build_stream::<i16>(&device, cfg, adapter),
                cpal::SampleFormat::U16 => build_stream::<u16>(&device, cfg, adapter),
                fmt => return Err(format!("unsupported sample format: {fmt}").into()),
            };
            match built {
                Ok(stream) => {
                    stream.play()?;
                    // Now open the matching input stream. If it fails, the
                    // server runs output-only (the engine reads silence).
                    let input_stream = match input_producer {
                        Some(tx) => match open_input(&host, inputs, rate, tx) {
                            Ok(s) => {
                                handle.input_channels = inputs;
                                Some(s)
                            }
                            Err(e) => {
                                tracing::warn!("no audio input ({inputs} ch): {e}");
                                handle.input_channels = 0;
                                None
                            }
                        },
                        None => None,
                    };
                    return Ok((
                        AudioBackend {
                            sample_rate: rate as f32,
                            channels: channels as usize,
                            input_channels: handle.input_channels,
                            _stream: stream,
                            _input_stream: input_stream,
                        },
                        handle,
                    ));
                }
                Err(e) => last_err = Some(e),
            }
        }
    }
    Err(last_err
        .map(|e| e.to_string())
        .unwrap_or_else(|| "could not open an output stream".to_string())
        .into())
}

/// Opens the default input device with `channels` channels at `rate` and
/// starts a callback that pushes interleaved f32 frames into `tx` for the
/// engine. Runs on the input callback thread (also real-time): only ring
/// pushes, in flush-to-zero mode. Returns the playing stream to keep alive.
fn open_input(
    host: &cpal::Host,
    channels: usize,
    rate: cpal::SampleRate,
    tx: Producer<f32>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let device = host
        .default_input_device()
        .ok_or("no input device available")?;
    let format = device.default_input_config()?.sample_format();
    let cfg = cpal::StreamConfig {
        channels: channels as u16,
        sample_rate: rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let stream = match format {
        cpal::SampleFormat::F32 => build_input_stream::<f32>(&device, cfg, tx),
        cpal::SampleFormat::I16 => build_input_stream::<i16>(&device, cfg, tx),
        cpal::SampleFormat::U16 => build_input_stream::<u16>(&device, cfg, tx),
        fmt => return Err(format!("unsupported input sample format: {fmt}").into()),
    }?;
    stream.play()?;
    Ok(stream)
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut tx: Producer<f32>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            crate::dsp::denormals::flush_to_zero();
            for &s in data.iter() {
                // Full ring = the engine is behind: drop the excess rather than
                // block on the RT callback thread.
                let _ = tx.push(f32::from_sample(s));
            }
        },
        |err| tracing::error!("input stream error: {err}"),
        None,
    )
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut adapter: BlockAdapter,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            // One-shot pinning/diagnostic of this thread (cold after boot).
            #[cfg(feature = "rtprio")]
            adapter.rt_setup.on_callback();
            // Subnormals in decaying DSP state are 10-100x slower: keep the
            // callback thread in flush-to-zero mode (see dsp::denormals).
            crate::dsp::denormals::flush_to_zero();
            for s in data.iter_mut() {
                *s = T::from_sample(adapter.next_sample());
            }
        },
        |err| tracing::error!("stream error: {err}"),
        None,
    )
}
