//! Real-time audio backend on top of cpal.
//!
//! cpal delivers interleaved buffers of variable size, not necessarily a
//! multiple of [`BLOCK_SIZE`]; `BlockAdapter` slices them up by requesting
//! blocks from the engine and keeping the leftover across callbacks.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

use crate::server::engine::{BLOCK_SIZE, Engine, EngineHandle, engine_pair_full};

pub struct AudioBackend {
    pub sample_rate: f32,
    pub channels: usize,
    // The stream stops when dropped: the backend must be kept alive.
    _stream: cpal::Stream,
}

struct BlockAdapter {
    engine: Engine,
    buf: Vec<f32>,
    pos: usize,
}

impl BlockAdapter {
    fn new(engine: Engine) -> Self {
        let len = BLOCK_SIZE * engine.channels();
        Self {
            engine,
            buf: vec![0.0; len],
            pos: len, // forces a process_block on the first sample
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
/// with `workers` M13 DSP threads for `/g_parallel` groups (0 = sequential)
/// and, optionally, an M14 IPC segment (shared clock + control buses).
/// Returns the handle the network thread uses to talk to the engine.
pub fn start(
    workers: usize,
    ipc: Option<std::sync::Arc<crate::server::ipc::Segment>>,
    requested_sample_rate: Option<u32>,
) -> Result<(AudioBackend, EngineHandle), Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no output device available")?;
    let default = device.default_output_config()?;
    let format = default.sample_format();
    let channels = default.channels();
    let device_rate = default.sample_rate();

    // Impose the requested rate by building the stream at it directly: PipeWire
    // honors arbitrary per-application rates (resampling to the graph rate
    // transparently), so we do not gate on `supported_output_configs`, which
    // under-reports there. Hosts that reject the rate (CoreAudio, WASAPI, plain
    // ALSA) make `build_*_stream` fail, and we fall back to the device's own
    // rate — the gap then shows up as `nominal != actual` in `/status.reply`.
    let rates = match requested_sample_rate {
        Some(hz) if hz != device_rate => [Some(hz), Some(device_rate)],
        _ => [Some(device_rate), None],
    };
    let mut last_err: Option<cpal::Error> = None;
    for rate in rates.into_iter().flatten() {
        let cfg = cpal::StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let (engine, handle) =
            engine_pair_full(rate as f32, channels as usize, workers, ipc.clone());
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
                return Ok((
                    AudioBackend {
                        sample_rate: rate as f32,
                        channels: channels as usize,
                        _stream: stream,
                    },
                    handle,
                ));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err
        .map(|e| e.to_string())
        .unwrap_or_else(|| "could not open an output stream".to_string())
        .into())
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
            // Subnormals in decaying DSP state are 10-100x slower: keep the
            // callback thread in flush-to-zero mode (see dsp::denormals).
            crate::dsp::denormals::flush_to_zero();
            for s in data.iter_mut() {
                *s = T::from_sample(adapter.next_sample());
            }
        },
        |err| eprintln!("stream error: {err}"),
        None,
    )
}
