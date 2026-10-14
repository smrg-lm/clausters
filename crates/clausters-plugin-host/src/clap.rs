//! CLAP adapter. Loaded only inside disposable/instance helper processes.

use crate::format::*;
use clack_common::events::event_types::*;
use clack_common::events::io::{EventBuffer, OutputEventBuffer, OutputEvents, TryPushError};
use clack_common::events::spaces::CoreEventSpace;
use clack_common::events::{EventFlags, EventHeader, Match, Pckn, UnknownEvent};
use clack_common::utils::{BeatTime, ClapId, SecondsTime};
use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, PluginGui};
use clack_extensions::latency::PluginLatency;
use clack_extensions::note_ports::{NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_extensions::state::PluginState;
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_host::prelude::*;
use clack_host::process::PluginAudioProcessor;
use clausters_core::plugin_shm::{Config, Event as WireEvent, EventKind, EventList, FRAMES, Span};
use std::any::Any;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, ThreadId};

struct Signals {
    main: ThreadId,
    audio: OnceLock<ThreadId>,
    callback: AtomicBool,
    restart: AtomicBool,
}
struct Shared(Arc<Signals>);
impl SharedHandler<'_> for Shared {
    fn request_restart(&self) {
        self.0.restart.store(true, Ordering::Release);
    }
    fn request_process(&self) {}
    fn request_callback(&self) {
        self.0.callback.store(true, Ordering::Release);
    }
}
impl HostThreadCheckImpl for Shared {
    fn is_main_thread(&self) -> bool {
        thread::current().id() == self.0.main
    }
    fn is_audio_thread(&self) -> bool {
        self.0.audio.get() == Some(&thread::current().id())
    }
}
struct Host;
impl HostHandlers for Host {
    type Shared<'a> = Shared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
    fn declare_extensions(builder: &mut HostExtensions<Self>, _: &Self::Shared<'_>) {
        builder.register::<HostThreadCheck>();
    }
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn ctext(value: Option<&CStr>) -> String {
    value
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn entry(path: &Path) -> Result<PluginEntry, String> {
    // SAFETY: external code is intentionally loaded only in the isolated helper.
    unsafe { PluginEntry::load(path) }.map_err(error)
}
pub(crate) fn scan(path: &Path, offset: usize) -> Result<Vec<ScanEntry>, String> {
    let entry = entry(path)?;
    let factory = entry.get_plugin_factory().ok_or("missing CLAP factory")?;
    let ids: Vec<_> = factory
        .plugin_descriptors()
        .take(MAX_PLUGINS + 1)
        .map(|d| d.id().map(CStr::to_owned).ok_or("missing CLAP plugin id"))
        .collect::<Result<_, _>>()?;
    if ids.len() > MAX_PLUGINS {
        return Err("plugin binary catalogue capacity exceeded".into());
    }
    let mut output = Vec::new();
    for id in ids.into_iter().skip(offset).take(4) {
        let result = (|| {
            let mut plugin = load(path, id.to_str().map_err(error)?)?;
            let info = plugin.metadata()?;
            let config = Config {
                generation: 1,
                sample_rate: 48000.0,
                sample_origin: 0,
                inputs: info.inputs.iter().map(|p| p.channels).sum(),
                outputs: info.outputs.iter().map(|p| p.channels).sum(),
                depth: 4,
                reserved: 0,
            };
            let processor = plugin.activate(config)?;
            let metadata = plugin.metadata()?;
            plugin.deactivate(processor.stop())?;
            Ok::<_, String>(metadata)
        })();
        output.push(match result {
            Ok(plugin) => ScanEntry::Plugin {
                plugin: Box::new(plugin),
            },
            Err(error) => ScanEntry::Error {
                path: path.to_string_lossy().into_owned(),
                error: format!("{}: {error}", id.to_string_lossy()),
            },
        });
    }
    Ok(output)
}
pub(crate) fn load(path: &Path, id: &str) -> Result<Box<dyn FormatInstance>, String> {
    let entry = entry(path)?;
    let id = CString::new(id).map_err(error)?;
    let descriptor = entry
        .get_plugin_factory()
        .ok_or("missing CLAP factory")?
        .plugin_descriptors()
        .take(MAX_PLUGINS + 1)
        .find(|d| d.id() == Some(id.as_c_str()))
        .ok_or("unknown CLAP plugin id")?;
    if !clack_common::utils::ClapVersion::from_raw(descriptor.as_raw().clap_version).is_compatible()
    {
        return Err("incompatible CLAP descriptor".into());
    }
    let signals = Arc::new(Signals {
        main: thread::current().id(),
        audio: OnceLock::new(),
        callback: AtomicBool::new(false),
        restart: AtomicBool::new(false),
    });
    let host = HostInfo::new(
        "Clausters",
        "Clausters",
        "https://github.com/smrg-lm/clausters",
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(error)?;
    let instance =
        PluginInstance::<Host>::new(|_| Shared(Arc::clone(&signals)), |_| (), &entry, &id, &host)
            .map_err(error)?;
    let plugin = Clap {
        instance,
        path: path.to_string_lossy().into_owned(),
        id: id.to_string_lossy().into_owned(),
        name: ctext(descriptor.name()),
        vendor: ctext(descriptor.vendor()),
        version: ctext(descriptor.version()),
        signals,
    };
    let mut plugin = Box::new(plugin);
    plugin.main_callback();
    plugin.metadata()?.validate()?;
    Ok(plugin)
}
struct Clap {
    instance: PluginInstance<Host>,
    path: String,
    id: String,
    name: String,
    vendor: String,
    version: String,
    signals: Arc<Signals>,
}
impl FormatInstance for Clap {
    fn metadata(&mut self) -> Result<PluginInfo, String> {
        let active = self.instance.is_active();
        let handle = self.instance.plugin_handle();
        let audio = |input| -> Result<Vec<AudioPort>, String> {
            let Some(ext) = handle.get_extension::<PluginAudioPorts>() else {
                return Ok(Vec::new());
            };
            let count = ext.count(&handle, input);
            if count > MAX_PORTS as u32 {
                return Err("plugin audio port capacity exceeded".into());
            }
            let mut buffer = AudioPortInfoBuffer::new();
            (0..count)
                .map(|i| {
                    let p = ext
                        .get(&handle, i, input, &mut buffer)
                        .ok_or("invalid CLAP audio port")?;
                    Ok(AudioPort {
                        id: p.id.get(),
                        name: text(p.name),
                        channels: p.channel_count,
                        flags: p.flags.bits(),
                    })
                })
                .collect()
        };
        let inputs = audio(true)?;
        let outputs = audio(false)?;
        let notes = |input| -> Result<Vec<NotePort>, String> {
            let Some(ext) = handle.get_extension::<PluginNotePorts>() else {
                return Ok(Vec::new());
            };
            let count = ext.count(&handle, input);
            if count > MAX_PORTS as u32 {
                return Err("plugin note port capacity exceeded".into());
            }
            let mut buffer = NotePortInfoBuffer::new();
            (0..count)
                .map(|i| {
                    let p = ext
                        .get(&handle, i, input, &mut buffer)
                        .ok_or("invalid CLAP note port")?;
                    Ok(NotePort {
                        id: p.id.get(),
                        name: text(p.name),
                        dialects: p.supported_dialects.bits(),
                    })
                })
                .collect()
        };
        let note_inputs = notes(true)?;
        let note_outputs = notes(false)?;
        let mut parameters = Vec::new();
        if let Some(ext) = handle.get_extension::<PluginParams>() {
            let count = ext.count(&handle);
            if count > MAX_PARAMETERS as u32 {
                return Err("plugin parameter capacity exceeded".into());
            }
            let mut buffer = ParamInfoBuffer::new();
            for i in 0..count {
                let p = ext
                    .get_info(&handle, i, &mut buffer)
                    .ok_or("invalid CLAP parameter")?;
                parameters.push(Parameter {
                    id: p.id.get(),
                    name: text(p.name),
                    module: text(p.module),
                    flags: p.flags.bits(),
                    min: p.min_value,
                    max: p.max_value,
                    default: p.default_value,
                });
            }
        }
        let latency = if active {
            handle
                .get_extension::<PluginLatency>()
                .map(|e| e.get(&handle))
                .unwrap_or(0)
        } else {
            0
        };
        let state = handle.get_extension::<PluginState>().is_some();
        let editor = handle.get_extension::<PluginGui>().is_some_and(|e| {
            e.is_api_supported(
                &handle,
                GuiConfiguration {
                    api_type: GuiApiType::X11,
                    is_floating: true,
                },
            )
        });
        let info = PluginInfo {
            format: "clap".into(),
            path: self.path.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            vendor: self.vendor.clone(),
            version: self.version.clone(),
            inputs,
            outputs,
            note_inputs,
            note_outputs,
            parameters,
            latency,
            state,
            editor,
        };
        info.validate()?;
        Ok(info)
    }
    fn activate(&mut self, config: Config) -> Result<Box<dyn FormatProcessor>, String> {
        let info = self.metadata()?;
        if !config.valid() || !info.matches(config) {
            return Err("plugin ports do not match segment".into());
        }
        let processor = self
            .instance
            .activate(
                |_, _| (),
                PluginAudioConfiguration {
                    sample_rate: config.sample_rate,
                    min_frames_count: 1,
                    max_frames_count: FRAMES as u32,
                },
            )
            .map_err(error)?;
        let buffers = |ports: &[AudioPort]| {
            ports
                .iter()
                .map(|p| (0..p.channels).map(|_| vec![0.0f32; FRAMES]).collect())
                .collect()
        };
        Ok(Box::new(Audio {
            processor: processor.into(),
            signals: Arc::clone(&self.signals),
            rate: config.sample_rate,
            input: buffers(&info.inputs),
            output: buffers(&info.outputs),
            in_ports: AudioPorts::with_capacity(config.inputs as usize, info.inputs.len()),
            out_ports: AudioPorts::with_capacity(config.outputs as usize, info.outputs.len()),
            input_events: EventBuffer::with_capacity(16),
            parameters: info.parameters,
            notes: info.note_inputs,
        }))
    }
    fn deactivate(&mut self, processor: Box<dyn Any + Send>) -> Result<(), String> {
        let stopped = processor
            .downcast::<StoppedPluginAudioProcessor<Host>>()
            .map_err(|_| "wrong format processor")?;
        self.instance.deactivate(*stopped);
        Ok(())
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        let handle = self.instance.plugin_handle();
        let ext = handle
            .get_extension::<PluginState>()
            .ok_or("plugin has no state extension")?;
        struct Bounded(Vec<u8>);
        impl std::io::Write for Bounded {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if b.len() > MAX_STATE_BYTES - self.0.len() {
                    return Err(std::io::Error::other("plugin state capacity exceeded"));
                }
                self.0.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Bounded(Vec::new());
        ext.save(&handle, &mut writer).map_err(error)?;
        Ok(writer.0)
    }
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err("plugin state capacity exceeded".into());
        }
        let handle = self.instance.plugin_handle();
        handle
            .get_extension::<PluginState>()
            .ok_or("plugin has no state extension")?
            .load(&handle, &mut &bytes[..])
            .map_err(error)
    }
    fn parameter(&mut self, id: u32) -> Result<f64, String> {
        let handle = self.instance.plugin_handle();
        handle
            .get_extension::<PluginParams>()
            .ok_or("plugin has no parameters")?
            .get_value(
                &handle,
                ClapId::from_raw(id).ok_or("invalid plugin parameter id")?,
            )
            .ok_or("unknown plugin parameter".into())
    }
    fn main_callback(&mut self) {
        if self.signals.callback.swap(false, Ordering::AcqRel) {
            self.instance.call_on_main_thread_callback();
        }
    }
    fn restart_requested(&self) -> bool {
        self.signals.restart.swap(false, Ordering::AcqRel)
    }
}
struct Audio {
    processor: PluginAudioProcessor<Host>,
    signals: Arc<Signals>,
    rate: f64,
    input: Vec<Vec<Vec<f32>>>,
    output: Vec<Vec<Vec<f32>>>,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    input_events: EventBuffer,
    parameters: Vec<Parameter>,
    notes: Vec<NotePort>,
}
fn pckn(e: WireEvent) -> Pckn {
    Pckn::new(
        if e.port >= 0 {
            Match::Specific(e.port as u16)
        } else {
            Match::All
        },
        if e.channel >= 0 {
            Match::Specific(e.channel as u16)
        } else {
            Match::All
        },
        if e.key >= 0 {
            Match::Specific(e.key as u16)
        } else {
            Match::All
        },
        if e.note_id >= 0 {
            Match::Specific(e.note_id as u32)
        } else {
            Match::All
        },
    )
}
impl FormatProcessor for Audio {
    fn start(&mut self) -> Result<(), String> {
        let _ = self.signals.audio.set(thread::current().id());
        self.processor.ensure_processing_started().map_err(error)?;
        Ok(())
    }
    fn pause(&mut self) {
        self.processor.ensure_processing_stopped();
    }
    fn stop(mut self: Box<Self>) -> Box<dyn Any + Send> {
        self.pause();
        Box::new(self.processor.into_stopped())
    }
    fn process(
        &mut self,
        span: Span,
        steady_time: u64,
        input: &[f32],
        output: &mut [f32],
        events: &EventList,
        result: &mut EventList,
    ) -> Result<(), String> {
        self.input_events.clear();
        for e in events.values.iter().take(events.count as usize).copied() {
            let k = pckn(e);
            if matches!(e.kind, 1..=3 | 6)
                && (e.port < 0
                    || self
                        .notes
                        .get(e.port as usize)
                        .is_none_or(|p| p.dialects & 1 == 0))
            {
                return Err("unsupported plugin note port/dialect".into());
            }
            match e.kind {
                1 => self
                    .input_events
                    .push(&NoteOnEvent::new(e.offset, k, e.value)),
                2 => self
                    .input_events
                    .push(&NoteOffEvent::new(e.offset, k, e.value)),
                3 => self.input_events.push(&NoteChokeEvent::new(e.offset, k)),
                4 => {
                    let p = self
                        .parameters
                        .iter()
                        .find(|p| p.id == e.id)
                        .ok_or("unknown plugin parameter")?;
                    if !(p.min..=p.max).contains(&e.value) || p.flags & (1 << 3) != 0 {
                        return Err("invalid plugin parameter value".into());
                    }
                    self.input_events.push(&ParamValueEvent::new(
                        e.offset,
                        ClapId::new(e.id),
                        k,
                        e.value,
                    ));
                }
                5 => {
                    if e.port < 0
                        || self
                            .notes
                            .get(e.port as usize)
                            .is_none_or(|p| p.dialects & 6 == 0)
                    {
                        return Err("unsupported plugin MIDI port/dialect".into());
                    }
                    self.input_events.push(&MidiEvent::new(
                        e.offset,
                        e.port as u16,
                        [e.id as u8, (e.id >> 8) as u8, (e.id >> 16) as u8],
                    ));
                }
                6 => self.input_events.push(&NoteExpressionEvent::new(
                    e.offset,
                    k,
                    NoteExpressionType::from_raw(e.id as i32)
                        .ok_or("unsupported note expression")?,
                    e.value,
                )),
                _ => return Err("unsupported plugin input event".into()),
            }
        }
        let n = span.frames as usize;
        let offset = span.offset as usize;
        for (channel, b) in self.input.iter_mut().flatten().enumerate() {
            b[..n]
                .copy_from_slice(&input[channel * FRAMES + offset..channel * FRAMES + offset + n]);
        }
        for b in self.output.iter_mut().flatten() {
            b[..n].fill(0.0);
        }
        let mut flags = TransportFlags::HAS_SECONDS_TIMELINE
            | TransportFlags::HAS_BEATS_TIMELINE
            | TransportFlags::HAS_TEMPO;
        if span.playing != 0 {
            flags |= TransportFlags::IS_PLAYING;
        }
        let transport = TransportEvent {
            header: EventHeader::new_core(0, EventFlags::empty()),
            flags,
            song_pos_seconds: SecondsTime::from_float(span.position as f64 / self.rate),
            tempo: span.tempo,
            song_pos_beats: BeatTime::from_float(
                span.position as f64 / self.rate * span.tempo / 60.0,
            ),
            tempo_inc: 0.0,
            loop_start_beats: BeatTime::from_bits(0),
            loop_end_beats: BeatTime::from_bits(0),
            loop_start_seconds: SecondsTime::from_bits(0),
            loop_end_seconds: SecondsTime::from_bits(0),
            bar_start: BeatTime::from_bits(0),
            bar_number: 0,
            time_signature_numerator: 0,
            time_signature_denominator: 0,
        };
        let mut collector = Collector {
            result,
            offset: span.offset,
            frames: span.frames,
            failed: false,
        };
        {
            let input_audio = self
                .in_ports
                .with_input_buffers(self.input.iter_mut().map(|port| AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_input_only(
                        port.iter_mut().map(|b| InputChannel::variable(&mut b[..n])),
                    ),
                }));
            let mut output_audio = self
                .out_ports
                .with_output_buffers(self.output.iter_mut().map(|port| AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_output_only(
                        port.iter_mut().map(|b| &mut b[..n]),
                    ),
                }));
            self.processor
                .as_started_mut()
                .map_err(error)?
                .process(
                    &input_audio,
                    &mut output_audio,
                    &self.input_events.as_input(),
                    &mut OutputEvents::from_buffer(&mut collector),
                    Some(steady_time),
                    Some(&transport),
                )
                .map_err(error)?;
        }
        if collector.failed {
            return Err("plugin output event unsupported or capacity exceeded".into());
        }
        for (channel, b) in self.output.iter().flatten().enumerate() {
            output[channel * FRAMES + offset..channel * FRAMES + offset + n]
                .copy_from_slice(&b[..n]);
        }
        Ok(())
    }
}
struct Collector<'a> {
    result: &'a mut EventList,
    offset: u32,
    frames: u32,
    failed: bool,
}
impl OutputEventBuffer for Collector<'_> {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let mut output = WireEvent {
            offset: event.header().time().saturating_add(self.offset),
            note_id: -1,
            port: -1,
            channel: -1,
            key: -1,
            ..WireEvent::default()
        };
        macro_rules! note {
            ($e:expr, $kind:expr) => {{
                let r = $e.as_raw();
                output.kind = $kind;
                output.note_id = r.note_id;
                output.port = r.port_index;
                output.channel = r.channel;
                output.key = r.key;
                output.value = r.velocity;
            }};
        }
        let valid = if event.header().time() >= self.frames {
            false
        } else {
            match event.as_core_event() {
                Some(CoreEventSpace::NoteOn(e)) => {
                    note!(e, EventKind::NoteOn as u32);
                    true
                }
                Some(CoreEventSpace::NoteOff(e)) => {
                    note!(e, EventKind::NoteOff as u32);
                    true
                }
                Some(CoreEventSpace::NoteChoke(e)) => {
                    note!(e, EventKind::NoteChoke as u32);
                    true
                }
                Some(CoreEventSpace::NoteEnd(e)) => {
                    note!(e, EventKind::NoteEnd as u32);
                    true
                }
                Some(CoreEventSpace::ParamValue(e)) => {
                    let r = e.as_raw();
                    output.kind = EventKind::Parameter as u32;
                    output.id = r.param_id;
                    output.note_id = r.note_id;
                    output.port = r.port_index;
                    output.channel = r.channel;
                    output.key = r.key;
                    output.value = r.value;
                    true
                }
                Some(CoreEventSpace::NoteExpression(e)) => {
                    let r = e.as_raw();
                    output.kind = EventKind::Expression as u32;
                    output.id = r.expression_id as u32;
                    output.note_id = r.note_id;
                    output.port = r.port_index;
                    output.channel = r.channel;
                    output.key = r.key;
                    output.value = r.value;
                    true
                }
                Some(CoreEventSpace::ParamMod(e)) => {
                    let r = e.as_raw();
                    output.kind = EventKind::ParameterMod as u32;
                    output.id = r.param_id;
                    output.note_id = r.note_id;
                    output.port = r.port_index;
                    output.channel = r.channel;
                    output.key = r.key;
                    output.value = r.amount;
                    true
                }
                Some(CoreEventSpace::ParamGestureBegin(e)) => {
                    output.kind = EventKind::GestureBegin as u32;
                    output.id = e.param_id().map_or(u32::MAX, |id| id.get());
                    output.id != u32::MAX
                }
                Some(CoreEventSpace::ParamGestureEnd(e)) => {
                    output.kind = EventKind::GestureEnd as u32;
                    output.id = e.param_id().map_or(u32::MAX, |id| id.get());
                    output.id != u32::MAX
                }
                Some(CoreEventSpace::Midi(e)) => {
                    let r = e.as_raw();
                    output.kind = EventKind::Midi as u32;
                    output.port = r.port_index as i16;
                    output.id =
                        r.data[0] as u32 | ((r.data[1] as u32) << 8) | ((r.data[2] as u32) << 16);
                    true
                }
                _ => false,
            }
        };
        if !valid || !self.result.push(output) {
            self.failed = true;
            return Err(TryPushError::new());
        }
        Ok(())
    }
}
