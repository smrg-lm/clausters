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
) -> Result<(AudioBackend, EngineHandle), Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no output device available")?;
    let config = device.default_output_config()?;

    let sample_rate = config.sample_rate() as f32;
    let channels = config.channels() as usize;
    let (engine, handle) = engine_pair_full(sample_rate, channels, workers, ipc);
    let adapter = BlockAdapter::new(engine);

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, config.into(), adapter)?,
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, config.into(), adapter)?,
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, config.into(), adapter)?,
        fmt => return Err(format!("unsupported sample format: {fmt}").into()),
    };
    stream.play()?;

    Ok((
        AudioBackend {
            sample_rate,
            channels,
            _stream: stream,
        },
        handle,
    ))
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
