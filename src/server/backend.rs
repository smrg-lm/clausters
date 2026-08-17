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

/// **Which devices this server holds, and under what name.**
///
/// An audio application is expected to say where its sound comes from and
/// where it goes, and to come back under the same name when it is restarted —
/// a patchbay reconnects by name, and a server whose ports are called
/// something new every run drops the user's routing every time. This is that
/// surface: `--host`, `--device`, `--input-device` and `--client-name`.
///
/// Every field is a *preference*: a name nothing matches is a warning and the
/// default device, never a refusal to start. Losing a device should not cost
/// the session.
#[derive(Debug, Default, Clone)]
pub struct Devices {
    /// The audio host (backend) to use, by name — `jack`, `alsa`, `pipewire`,
    /// `coreaudio`, `wasapi`, whatever this build has. `None` takes cpal's
    /// default, which prefers PipeWire on Linux where it is compiled in.
    pub host: Option<String>,
    /// The output device, by name as [`Self::list`] prints it.
    pub output: Option<String>,
    /// The input device, by name. Only opened when `--inputs` asks for
    /// channels: capture belongs to whoever holds the input device, and this
    /// is the process that does.
    pub input: Option<String>,
    /// What this server calls itself to the audio graph.
    ///
    /// PipeWire reads its properties from the environment, which is the only
    /// door cpal leaves open — without one its nodes are named
    /// `cpal-playback-<pid>`, so a restarted server never gets its ports back
    /// and every connection a person made is lost. Under JACK the client name
    /// comes from the *device* instead, so `--device` is what names it there.
    pub client_name: Option<String>,
}

impl Devices {
    /// Applies what has to be in place **before** the host is created: the
    /// PipeWire node name, which is read from the environment at connect time.
    ///
    /// An environment that already names the application is left alone — the
    /// person who set `PIPEWIRE_PROPS` meant it.
    pub fn arm(&self) {
        let Some(name) = &self.client_name else {
            return;
        };
        if std::env::var_os("PIPEWIRE_PROPS").is_some() {
            return;
        }
        // SAFETY: called before any audio host exists and before any thread
        // that could be reading the environment is spawned.
        unsafe {
            std::env::set_var(
                "PIPEWIRE_PROPS",
                format!("{{ application.name = {name} node.name = {name} }}"),
            );
        }
    }

    /// Every host and device this build can see, as lines to print — what
    /// `--list-devices` answers, and where the names the other flags take come
    /// from.
    pub fn list() -> Vec<String> {
        let mut out = Vec::new();
        for id in cpal::available_hosts() {
            let Ok(host) = cpal::host_from_id(id) else {
                out.push(format!("{} (unavailable)", id.name()));
                continue;
            };
            out.push(format!("{}:", id.name()));
            let default_out = host
                .default_output_device()
                .and_then(|d| d.description().ok().map(|d| d.name().to_string()));
            let default_in = host
                .default_input_device()
                .and_then(|d| d.description().ok().map(|d| d.name().to_string()));
            if let Ok(devices) = host.output_devices() {
                for name in
                    devices.filter_map(|d| d.description().ok().map(|d| d.name().to_string()))
                {
                    let mark = if Some(&name) == default_out.as_ref() {
                        " (default)"
                    } else {
                        ""
                    };
                    out.push(format!("  out  {name}{mark}"));
                }
            }
            if let Ok(devices) = host.input_devices() {
                for name in
                    devices.filter_map(|d| d.description().ok().map(|d| d.name().to_string()))
                {
                    let mark = if Some(&name) == default_in.as_ref() {
                        " (default)"
                    } else {
                        ""
                    };
                    out.push(format!("  in   {name}{mark}"));
                }
            }
        }
        out
    }
}

/// Picks the host `want` names, or cpal's default when it names nothing this
/// build has.
fn pick_host(want: Option<&String>) -> cpal::Host {
    let Some(want) = want else {
        return cpal::default_host();
    };
    for id in cpal::available_hosts() {
        if id.name().eq_ignore_ascii_case(want)
            && let Ok(host) = cpal::host_from_id(id)
        {
            return host;
        }
    }
    tracing::warn!("no audio host called {want:?} in this build; using the default");
    cpal::default_host()
}

/// Finds a device by name among `devices`, exactly first and then by a
/// case-insensitive substring — device names are long and a person types the
/// part that identifies theirs.
fn pick_device(
    devices: impl Iterator<Item = cpal::Device>,
    want: &str,
    role: &str,
) -> Option<cpal::Device> {
    let mut candidates: Vec<(String, cpal::Device)> = devices
        .filter_map(|d| {
            d.description()
                .ok()
                .map(|desc| (desc.name().to_string(), d))
        })
        .collect();
    if let Some(i) = candidates.iter().position(|(n, _)| n == want) {
        return Some(candidates.swap_remove(i).1);
    }
    let lower = want.to_lowercase();
    if let Some(i) = candidates
        .iter()
        .position(|(n, _)| n.to_lowercase().contains(&lower))
    {
        let (name, device) = candidates.swap_remove(i);
        tracing::info!("{role} device {want:?} matched {name:?}");
        return Some(device);
    }
    tracing::warn!("no {role} device matching {want:?}; using the default");
    None
}

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
/// with `workers` DSP threads for `/group_parallel` groups (0 = sequential),
/// the boot-time pool `limits`, and, optionally, an IPC segment (shared
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
    devices: &Devices,
) -> Result<(AudioBackend, EngineHandle), Box<dyn std::error::Error>> {
    devices.arm();
    let host = pick_host(devices.host.as_ref());
    let device = devices
        .output
        .as_deref()
        .and_then(|want| {
            host.output_devices()
                .ok()
                .and_then(|d| pick_device(d, want, "output"))
        })
        .or_else(|| host.default_output_device())
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
                        Some(tx) => {
                            match open_input(&host, inputs, rate, tx, devices.input.as_deref()) {
                                Ok(s) => {
                                    handle.input_channels = inputs;
                                    Some(s)
                                }
                                Err(e) => {
                                    tracing::warn!("no audio input ({inputs} ch): {e}");
                                    handle.input_channels = 0;
                                    None
                                }
                            }
                        }
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
    want: Option<&str>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let device = want
        .and_then(|want| {
            host.input_devices()
                .ok()
                .and_then(|d| pick_device(d, want, "input"))
        })
        .or_else(|| host.default_input_device())
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
