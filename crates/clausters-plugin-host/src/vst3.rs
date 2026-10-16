//! VST3 COM adapter, instantiated only in isolated Linux helpers.
//!
//! Processing uses VST3's native normalized parameter domain. Controller calls,
//! connections, state streams, activation and editor discovery stay on main.
// The binding spells its external interface methods verbatim.
#![allow(non_snake_case)]

#[path = "vst3/message.rs"]
mod message;
use crate::editor::{self, Timer, Window, WindowEvent};
use crate::format::*;
use ::vst3::Steinberg::Linux::*;
use ::vst3::{Class, ComPtr, ComWrapper, Interface, Steinberg::Vst::*, Steinberg::*};
use clausters_core::plugin_shm::{
    Config, EVENTS, Event as WireEvent, EventKind, EventList, FRAMES, Span,
};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

fn ok(result: tresult) -> Result<(), String> {
    if result == kResultOk {
        Ok(())
    } else {
        Err(format!("VST3 result {result}"))
    }
}
fn wide(value: &[u16]) -> String {
    String::from_utf16_lossy(
        &value
            .iter()
            .take_while(|&&v| v != 0)
            .copied()
            .collect::<Vec<_>>(),
    )
}
fn narrow(value: &[i8]) -> String {
    String::from_utf8_lossy(
        &value
            .iter()
            .take_while(|&&v| v != 0)
            .map(|&v| v as u8)
            .collect::<Vec<_>>(),
    )
    .into_owned()
}
fn identity(cid: TUID) -> String {
    cid.iter().map(|v| format!("{:02X}", *v as u8)).collect()
}
fn zero<T>() -> T {
    // SAFETY: only called for SDK numeric C records with no references/enums.
    unsafe { std::mem::zeroed() }
}

struct Module {
    factory: Option<ComPtr<IPluginFactory>>,
    exit: unsafe extern "system" fn() -> bool,
    _library: libloading::Library,
}
impl Drop for Module {
    fn drop(&mut self) {
        drop(self.factory.take());
        // SAFETY: all plugin interfaces are released before the module/library.
        unsafe {
            (self.exit)();
        }
    }
}
fn binary(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_owned());
    }
    let directory = path.join(format!("Contents/{}-linux", std::env::consts::ARCH));
    let expected = directory.join(format!(
        "{}.so",
        path.file_stem()
            .ok_or("invalid VST3 bundle")?
            .to_string_lossy()
    ));
    if expected.is_file() {
        return Ok(expected);
    }
    let files = std::fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "so"))
        .collect::<Vec<_>>();
    if files.len() == 1 {
        Ok(files[0].clone())
    } else {
        Err("VST3 bundle must identify one native binary".into())
    }
}
fn module(path: &Path) -> Result<Module, String> {
    // SAFETY: external code is deliberately confined to a disposable helper.
    unsafe {
        let library = libloading::os::unix::Library::open(
            Some(binary(path)?),
            // TLS destructors can outlive ModuleExit. Each helper owns at
            // most one processing module and exits when its node is freed.
            libc::RTLD_NOW | libc::RTLD_LOCAL | libc::RTLD_NODELETE,
        )
        .map_err(|e| e.to_string())?;
        let entry = *library
            .get::<unsafe extern "system" fn(*mut c_void) -> bool>(b"ModuleEntry\0")
            .map_err(|e| e.to_string())?;
        let exit = *library
            .get::<unsafe extern "system" fn() -> bool>(b"ModuleExit\0")
            .map_err(|e| e.to_string())?;
        let factory = *library
            .get::<unsafe extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory\0")
            .map_err(|e| e.to_string())?;
        let handle = library.into_raw();
        let library = libloading::os::unix::Library::from_raw(handle);
        if !entry(handle) {
            return Err("VST3 ModuleEntry failed".into());
        }
        let mut module = Module {
            factory: None,
            exit,
            _library: library.into(),
        };
        module.factory = Some(ComPtr::from_raw(factory()).ok_or("missing VST3 factory")?);
        Ok(module)
    }
}
fn classes(module: &Module) -> Result<Vec<PClassInfo>, String> {
    // SAFETY: valid retained factory; output records have the SDK layout.
    unsafe {
        let factory = module.factory.as_ref().unwrap();
        let count = factory.countClasses();
        if !(0..=MAX_PLUGINS as i32 * 2).contains(&count) {
            return Err("VST3 catalogue capacity exceeded".into());
        }
        let mut classes = Vec::new();
        for index in 0..count {
            let mut info = zero();
            ok(factory.getClassInfo(index, &mut info))?;
            if narrow(&info.category) == "Audio Module Class" {
                classes.push(info);
            }
        }
        if classes.len() > MAX_PLUGINS {
            return Err("VST3 catalogue capacity exceeded".into());
        }
        Ok(classes)
    }
}
unsafe fn create<I: Interface>(
    factory: &ComPtr<IPluginFactory>,
    cid: &TUID,
) -> Result<ComPtr<I>, String> {
    let mut output = ptr::null_mut();
    // SAFETY: factory receives SDK class/interface ids and one writable pointer.
    unsafe {
        ok(factory.createInstance(cid.as_ptr(), I::IID.as_ptr() as *const i8, &mut output))?;
        ComPtr::from_raw(output.cast()).ok_or("missing VST3 interface".into())
    }
}

struct Shared {
    restart: AtomicBool,
    /// The controller changed parameter values itself (a preset, a state).
    values_changed: AtomicBool,
    values: Vec<(u32, AtomicU64)>,
}
struct Host {
    shared: Arc<Shared>,
    edits: RefCell<rtrb::Producer<WireEvent>>,
    windows: Rc<RefCell<Windows>>,
}
impl Class for Host {
    type Interfaces = (IHostApplication, IComponentHandler, IPlugFrame, IRunLoop);
}
impl IHostApplicationTrait for Host {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        unsafe {
            *name = [0; 128];
            for (dst, src) in (*name).iter_mut().zip("Clausters".encode_utf16()) {
                *dst = src;
            }
        }
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        unsafe { message::create(cid, iid, obj) }
    }
}
#[derive(Default)]
struct Windows {
    window: Option<Window>,
    view: Option<ComPtr<IPlugView>>,
    timers: Vec<Timer<ComPtr<ITimerHandler>>>,
    fds: Vec<(i32, ComPtr<IEventHandler>)>,
}
impl IPlugFrameTrait for Host {
    unsafe fn resizeView(&self, view: *mut IPlugView, rect: *mut ViewRect) -> tresult {
        if rect.is_null() || view.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: VST3 owns both call arguments for the duration of this callback.
        let (width, height) =
            unsafe { ((*rect).right - (*rect).left, (*rect).bottom - (*rect).top) };
        let owned = {
            let mut windows = self.windows.borrow_mut();
            if windows.view.as_ref().is_none_or(|v| v.as_ptr() != view) {
                return kInvalidArgument;
            }
            if let Some(w) = &mut windows.window {
                w.resize(width as u32, height as u32).is_ok()
            } else {
                false
            }
        };
        if !owned {
            return kInvalidArgument;
        }
        // Release the RefCell before onSize: a plugin may reenter its frame.
        unsafe {
            let v = ComPtr::from_raw(view).unwrap();
            let result = v.onSize(rect);
            std::mem::forget(v);
            result
        }
    }
}
impl IRunLoopTrait for Host {
    unsafe fn registerEventHandler(&self, handler: *mut IEventHandler, fd: i32) -> tresult {
        if handler.is_null() || fd < 0 {
            return kInvalidArgument;
        }
        let mut windows = self.windows.borrow_mut();
        if windows.fds.len() >= editor::CALLBACKS || windows.fds.iter().any(|p| p.0 == fd) {
            return kInvalidArgument;
        }
        // SAFETY: plugin provides a live COM object; retain our own reference.
        unsafe {
            let h = ComPtr::from_raw(handler).unwrap();
            windows.fds.push((fd, h.clone()));
            std::mem::forget(h);
        }
        kResultOk
    }
    unsafe fn unregisterEventHandler(&self, handler: *mut IEventHandler) -> tresult {
        let mut windows = self.windows.borrow_mut();
        let n = windows.fds.len();
        windows.fds.retain(|p| p.1.as_ptr() != handler);
        if windows.fds.len() == n {
            kInvalidArgument
        } else {
            kResultOk
        }
    }
    unsafe fn registerTimer(&self, handler: *mut ITimerHandler, milliseconds: u64) -> tresult {
        if handler.is_null() {
            return kInvalidArgument;
        }
        let mut windows = self.windows.borrow_mut();
        if windows.timers.len() >= editor::CALLBACKS
            || windows.timers.iter().any(|p| p.id.as_ptr() == handler)
        {
            return kInvalidArgument;
        }
        // SAFETY: plugin provides a live COM object; retain our own reference.
        let h = unsafe { ComPtr::from_raw(handler).unwrap() };
        let timer = Timer::new(h.clone(), milliseconds);
        std::mem::forget(h);
        match timer {
            Ok(t) => {
                windows.timers.push(t);
                kResultOk
            }
            Err(_) => kInvalidArgument,
        }
    }
    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> tresult {
        let mut windows = self.windows.borrow_mut();
        let n = windows.timers.len();
        windows.timers.retain(|p| p.id.as_ptr() != handler);
        if windows.timers.len() == n {
            kInvalidArgument
        } else {
            kResultOk
        }
    }
}
impl IComponentHandlerTrait for Host {
    unsafe fn beginEdit(&self, id: u32) -> tresult {
        self.edit(id, 0.0, EventKind::GestureBegin)
    }
    unsafe fn endEdit(&self, id: u32) -> tresult {
        self.edit(id, 0.0, EventKind::GestureEnd)
    }
    unsafe fn performEdit(&self, id: u32, value: f64) -> tresult {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return kInvalidArgument;
        }
        if let Some((_, target)) = self.shared.values.iter().find(|p| p.0 == id) {
            target.store(value.to_bits(), Ordering::Release);
            self.edit(id, value, EventKind::Parameter)
        } else {
            kInvalidArgument
        }
    }
    /// Only a change the live surface cannot absorb needs explicit recovery:
    /// a reload, new buses, a new latency or a new MIDI controller map. New
    /// parameter values are read back; new titles or a remapped parameter ID
    /// table (a migration aid hosts consult for saved automation) change no
    /// identity a node holds. Instruments raise those routinely.
    unsafe fn restartComponent(&self, flags: i32) -> tresult {
        const RELOAD: i32 = 1;
        const IO: i32 = 1 << 1;
        const VALUES: i32 = 1 << 2;
        const LATENCY: i32 = 1 << 3;
        const MIDI_CC: i32 = 1 << 5;
        if flags & (RELOAD | IO | LATENCY | MIDI_CC) != 0 {
            self.shared.restart.store(true, Ordering::Release);
        }
        if flags & VALUES != 0 {
            self.shared.values_changed.store(true, Ordering::Release);
        }
        kResultOk
    }
}

// State streams never cross the audio seam. Writes and seeks are bounded.
struct Stream {
    bytes: RefCell<Vec<u8>>,
    position: Cell<usize>,
    failed: Cell<bool>,
}
impl Class for Stream {
    type Interfaces = (IBStream,);
}
impl Stream {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: RefCell::new(bytes),
            position: Cell::new(0),
            failed: Cell::new(false),
        }
    }
}
impl IBStreamTrait for Stream {
    unsafe fn read(&self, buffer: *mut c_void, count: i32, done: *mut i32) -> tresult {
        if count < 0 || (count > 0 && buffer.is_null()) {
            return kInvalidArgument;
        }
        let bytes = self.bytes.borrow();
        let position = self.position.get();
        let n = (count as usize).min(bytes.len().saturating_sub(position));
        if n > 0 {
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr().add(position), buffer.cast(), n);
            }
        }
        self.position.set(position + n);
        if !done.is_null() {
            unsafe {
                *done = n as i32;
            }
        }
        if n == count as usize {
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn write(&self, buffer: *mut c_void, count: i32, done: *mut i32) -> tresult {
        if count < 0 || (count > 0 && buffer.is_null()) {
            return kInvalidArgument;
        }
        let end = self.position.get().saturating_add(count as usize);
        if end > MAX_STATE_BYTES {
            self.failed.set(true);
            return kResultFalse;
        }
        let mut bytes = self.bytes.borrow_mut();
        let length = bytes.len().max(end);
        bytes.resize(length, 0);
        if count > 0 {
            unsafe {
                ptr::copy_nonoverlapping(
                    buffer.cast(),
                    bytes.as_mut_ptr().add(self.position.get()),
                    count as usize,
                );
            }
        }
        self.position.set(end);
        if !done.is_null() {
            unsafe {
                *done = count;
            }
        }
        kResultOk
    }
    unsafe fn seek(&self, position: i64, mode: i32, result: *mut i64) -> tresult {
        let base = match mode {
            0 => 0,
            1 => self.position.get() as i64,
            2 => self.bytes.borrow().len() as i64,
            _ => return kInvalidArgument,
        };
        let Some(target) = base
            .checked_add(position)
            .filter(|p| (0..=MAX_STATE_BYTES as i64).contains(p))
        else {
            self.failed.set(true);
            return kInvalidArgument;
        };
        self.position.set(target as usize);
        if !result.is_null() {
            unsafe {
                *result = target;
            }
        }
        kResultOk
    }
    unsafe fn tell(&self, result: *mut i64) -> tresult {
        if result.is_null() {
            return kInvalidArgument;
        }
        unsafe {
            *result = self.position.get() as i64;
        }
        kResultOk
    }
}

impl Host {
    fn edit(&self, id: u32, value: f64, kind: EventKind) -> tresult {
        if !self.shared.values.iter().any(|p| p.0 == id) {
            return kInvalidArgument;
        }
        let event = WireEvent {
            kind: kind as u32,
            id,
            value,
            note_id: -1,
            port: -1,
            channel: -1,
            key: -1,
            ..WireEvent::default()
        };
        if self.edits.borrow_mut().push(event).is_err() {
            self.shared.restart.store(true, Ordering::Release);
            kResultFalse
        } else {
            kResultOk
        }
    }
}
struct Instance {
    component: ComPtr<IComponent>,
    controller: ComPtr<IEditController>,
    processor: ComPtr<IAudioProcessor>,
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    edits: Option<rtrb::Consumer<WireEvent>>,
    separate: bool,
    active: bool,
    info: PluginInfo,
    shared: Arc<Shared>,
    windows: Rc<RefCell<Windows>>,
    _host: ComWrapper<Host>,
    handler: Option<ComWrapper<Host>>,
    _module: Module,
}
impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.editor(EditorCommand::Close);
        self.windows.borrow_mut().timers.clear();
        self.windows.borrow_mut().fds.clear();
        // SAFETY: helper returns the stopped processor before releasing main ownership.
        unsafe {
            if self.active {
                let _ = self.component.setActive(0);
            }
            if let Some((a, b)) = &self.connection {
                let _ = a.disconnect(b.as_ptr());
                let _ = b.disconnect(a.as_ptr());
            }
            let _ = self.controller.setComponentHandler(ptr::null_mut());
            if self.separate {
                let _ = self.controller.terminate();
            }
            let _ = self.component.terminate();
        }
    }
}
pub(crate) fn load(path: &Path, id: &str) -> Result<Box<dyn FormatInstance>, String> {
    let module = module(path)?;
    let class = classes(&module)?
        .into_iter()
        .find(|c| identity(c.cid).eq_ignore_ascii_case(id))
        .ok_or("unknown VST3 class id")?;
    let factory = module.factory.as_ref().unwrap();
    // SAFETY: all lifecycle and controller calls happen on helper main.
    unsafe {
        let component: ComPtr<IComponent> = create(factory, &class.cid)?;
        let shared = Arc::new(Shared {
            restart: AtomicBool::new(false),
            values_changed: AtomicBool::new(false),
            values: (0..MAX_PARAMETERS)
                .map(|_| (u32::MAX, AtomicU64::new(0)))
                .collect(),
        });
        let (tx, rx) = rtrb::RingBuffer::new(EVENTS);
        let windows = Rc::new(RefCell::new(Windows::default()));
        let host = ComWrapper::new(Host {
            shared: Arc::clone(&shared),
            edits: RefCell::new(tx),
            windows: Rc::clone(&windows),
        });
        let context = host.to_com_ptr::<IHostApplication>().unwrap();
        ok(component.initialize(context.as_ptr().cast()))?;
        let processor = component
            .cast::<IAudioProcessor>()
            .ok_or("missing VST3 audio processor")?;
        let (controller, separate) = if let Some(controller) = component.cast::<IEditController>() {
            (controller, false)
        } else {
            let mut cid = [0; 16];
            ok(component.getControllerClassId(&mut cid))?;
            let controller: ComPtr<IEditController> = create(factory, &cid)?;
            ok(controller.initialize(context.as_ptr().cast()))?;
            (controller, true)
        };
        ok(controller
            .setComponentHandler(host.to_com_ptr::<IComponentHandler>().unwrap().as_ptr()))?;
        let connection = component
            .cast::<IConnectionPoint>()
            .zip(controller.cast::<IConnectionPoint>());
        if let Some((a, b)) = &connection {
            ok(a.connect(b.as_ptr()))?;
            ok(b.connect(a.as_ptr()))?;
        }
        let mut vendor: PFactoryInfo = zero();
        ok(factory.getFactoryInfo(&mut vendor))?;
        let mut version = String::new();
        let mut features = Vec::new();
        let mut sdk_version = String::new();
        let mut class_vendor = String::new();
        if let Some(factory) = factory.cast::<IPluginFactory2>() {
            for index in 0..factory.countClasses() {
                let mut extended: PClassInfo2 = zero();
                if factory.getClassInfo2(index, &mut extended) == kResultOk
                    && extended.cid == class.cid
                {
                    version = narrow(&extended.version);
                    sdk_version = narrow(&extended.sdkVersion);
                    class_vendor = narrow(&extended.vendor);
                    features = narrow(&extended.subCategories)
                        .split('|')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    break;
                }
            }
        }
        let info = PluginInfo {
            format: "vst3".into(),
            path: path
                .canonicalize()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
            id: identity(class.cid),
            name: narrow(&class.name),
            vendor: if class_vendor.is_empty() {
                narrow(&vendor.vendor)
            } else {
                class_vendor
            },
            version,
            description: String::new(),
            url: narrow(&vendor.url),
            manual_url: String::new(),
            support_url: narrow(&vendor.email),
            features,
            sdk_version,
            inputs: vec![],
            outputs: vec![],
            note_inputs: vec![],
            note_outputs: vec![],
            parameters: vec![],
            latency: 0,
            state: true,
            editor: false,
        };
        let mut instance = Instance {
            component,
            controller,
            processor,
            connection,
            edits: Some(rx),
            separate,
            active: false,
            info,
            shared,
            windows,
            _host: host,
            handler: None,
            _module: module,
        };
        // Replace the handler before handing any processing object to audio.
        let parameters = instance.parameters()?;
        let shared = Arc::new(Shared {
            restart: AtomicBool::new(false),
            values_changed: AtomicBool::new(false),
            values: parameters
                .iter()
                .map(|p| {
                    (
                        p.id,
                        AtomicU64::new(instance.controller.getParamNormalized(p.id).to_bits()),
                    )
                })
                .collect(),
        });
        let (tx, rx) = rtrb::RingBuffer::new(EVENTS);
        let host = ComWrapper::new(Host {
            shared: Arc::clone(&shared),
            edits: RefCell::new(tx),
            windows: Rc::clone(&instance.windows),
        });
        ok(instance
            .controller
            .setComponentHandler(host.to_com_ptr::<IComponentHandler>().unwrap().as_ptr()))?;
        // Keep the initialize context alive as well as the replacement handler.
        instance.shared = shared;
        instance.edits = Some(rx);
        instance.handler = Some(host);
        instance.sync_component()?;
        instance.metadata()?;
        Ok(Box::new(instance))
    }
}
/// A bus's channel layout from its speaker arrangement, else from its count.
fn layout(arrangement: Option<SpeakerArrangement>, channels: i32) -> String {
    use SpeakerArr::*;
    let ambisonic = [
        kAmbi1stOrderACN,
        kAmbi2cdOrderACN,
        kAmbi3rdOrderACN,
        kAmbi4thOrderACN,
        kAmbi5thOrderACN,
        kAmbi6thOrderACN,
        kAmbi7thOrderACN,
    ];
    match (arrangement, channels) {
        (Some(a), _) if a == kMono => "mono",
        (None, 1) => "mono",
        (Some(a), _) if ambisonic.contains(&a) => "ambisonic",
        (_, 2) => "stereo",
        (_, c) if c > 2 => "surround",
        _ => "",
    }
    .into()
}
impl Instance {
    /// Each unit's path of names from the root, which a parameter's module is;
    /// empty without unit information.
    fn units(&self) -> std::collections::HashMap<i32, String> {
        let mut paths = std::collections::HashMap::new();
        let Some(units) = self.controller.cast::<IUnitInfo>() else {
            return paths;
        };
        let mut found = std::collections::HashMap::new();
        // SAFETY: helper main thread, as every controller call.
        unsafe {
            for index in 0..units.getUnitCount().clamp(0, 4096) {
                let mut unit: UnitInfo = zero();
                if units.getUnitInfo(index, &mut unit) == kResultOk {
                    found.insert(unit.id, (unit.parentUnitId, wide(&unit.name)));
                }
            }
        }
        for &id in found.keys() {
            let mut names = Vec::new();
            let mut at = id;
            while at != kRootUnitId && names.len() < 32 {
                let Some((parent, name)) = found.get(&at) else {
                    break;
                };
                names.push(name.clone());
                at = *parent;
            }
            names.reverse();
            paths.insert(id, names.join("/"));
        }
        paths
    }
    fn parameters(&self) -> Result<Vec<Parameter>, String> {
        let units = self.units();
        unsafe {
            let count = self.controller.getParameterCount();
            if !(0..=MAX_PARAMETERS as i32).contains(&count) {
                return Err("VST3 parameter capacity exceeded".into());
            }
            let mut parameters = Vec::new();
            for index in 0..count {
                let mut p: ParameterInfo = zero();
                ok(self.controller.getParameterInfo(index, &mut p))?;
                if p.stepCount < 0 {
                    return Err("invalid VST3 parameter step count".into());
                }
                // Common metadata flag vocabulary is CLAP's; never forward VST3
                // bit 3 (program change) as CLAP's read-only flag.
                use ParameterInfo_::ParameterFlags_::{
                    kCanAutomate, kIsHidden, kIsList, kIsReadOnly,
                };
                let list = p.stepCount > 0 && p.flags & kIsList != 0;
                let mut flags = 0;
                for (on, flag) in [
                    (p.stepCount > 0, Parameter::STEPPED),
                    (p.flags & kIsReadOnly != 0, Parameter::READ_ONLY),
                    (p.flags & kCanAutomate != 0, Parameter::AUTOMATABLE),
                    (p.flags & kIsHidden != 0, Parameter::HIDDEN),
                    (list, Parameter::LIST),
                ] {
                    if on {
                        flags |= flag;
                    }
                }
                parameters.push(Parameter {
                    id: p.id,
                    name: wide(&p.title),
                    module: units.get(&p.unitId).cloned().unwrap_or_default(),
                    flags,
                    min: 0.0,
                    max: 1.0,
                    step: if p.stepCount > 0 {
                        1.0 / f64::from(p.stepCount)
                    } else {
                        0.0
                    },
                    default: p.defaultNormalizedValue,
                    // A list's steps are named once here, at discovery, so a
                    // generic view shows them without asking the plugin again.
                    labels: if list && (p.stepCount as usize) < MAX_LABELS {
                        (0..=p.stepCount)
                            .map(|n| {
                                let mut text = [0u16; 128];
                                let at = f64::from(n) / f64::from(p.stepCount);
                                if self.controller.getParamStringByValue(p.id, at, &mut text)
                                    == kResultOk
                                {
                                    wide(&text)
                                } else {
                                    String::new()
                                }
                            })
                            .collect()
                    } else {
                        Vec::new()
                    },
                });
            }
            Ok(parameters)
        }
    }
    fn sync_component(&mut self) -> Result<(), String> {
        let stream = ComWrapper::new(Stream::new(vec![]));
        unsafe {
            if self
                .component
                .getState(stream.to_com_ptr::<IBStream>().unwrap().as_ptr())
                == kResultOk
            {
                stream.position.set(0);
                let result = self
                    .controller
                    .setComponentState(stream.to_com_ptr::<IBStream>().unwrap().as_ptr());
                if result != kNotImplemented {
                    ok(result)?;
                }
            }
        }
        Ok(())
    }
    fn refresh_values(&self) {
        for (id, value) in &self.shared.values {
            unsafe {
                value.store(
                    self.controller.getParamNormalized(*id).to_bits(),
                    Ordering::Release,
                );
            }
        }
    }
}
impl FormatInstance for Instance {
    fn metadata(&mut self) -> Result<PluginInfo, String> {
        unsafe {
            let mut info = self.info.clone();
            info.inputs.clear();
            info.outputs.clear();
            info.note_inputs.clear();
            info.note_outputs.clear();
            for direction in 0..=1 {
                for media in 0..=1 {
                    let count = self.component.getBusCount(media, direction);
                    if !(0..=MAX_PORTS as i32).contains(&count) {
                        return Err("VST3 port capacity exceeded".into());
                    }
                    for index in 0..count {
                        let mut bus: BusInfo = zero();
                        ok(self.component.getBusInfo(media, direction, index, &mut bus))?;
                        if media == 0 {
                            if bus.channelCount <= 0 {
                                return Err("invalid VST3 audio bus".into());
                            }
                            let mut arrangement: SpeakerArrangement = 0;
                            let arranged = self.processor.getBusArrangement(
                                direction,
                                index,
                                &mut arrangement,
                            ) == kResultOk;
                            let port = AudioPort {
                                id: index as u32,
                                name: wide(&bus.name),
                                channels: bus.channelCount as u32,
                                flags: bus.flags,
                                main: bus.busType == BusTypes_::kMain as BusType,
                                layout: layout(arranged.then_some(arrangement), bus.channelCount),
                                active: bus.flags & BusInfo_::BusFlags_::kDefaultActive != 0,
                            };
                            if direction == 0 {
                                info.inputs.push(port);
                            } else {
                                info.outputs.push(port);
                            }
                        } else {
                            let port = NotePort {
                                id: index as u32,
                                name: wide(&bus.name),
                                dialects: 3,
                                preferred: 1,
                                channels: bus.channelCount.max(0) as u32,
                            };
                            if direction == 0 {
                                info.note_inputs.push(port);
                            } else {
                                info.note_outputs.push(port);
                            }
                        }
                    }
                }
            }
            info.parameters = self.parameters()?;
            info.latency = self.processor.getLatencySamples();
            if let Some(view) = ComPtr::from_raw(self.controller.createView(c"editor".as_ptr())) {
                ok(view.setFrame(
                    self.handler
                        .as_ref()
                        .unwrap()
                        .to_com_ptr::<IPlugFrame>()
                        .unwrap()
                        .as_ptr(),
                ))?;
                info.editor =
                    view.isPlatformTypeSupported(c"X11EmbedWindowID".as_ptr()) == kResultOk;
                let _ = view.setFrame(ptr::null_mut());
            }
            info.validate()?;
            self.info = info.clone();
            Ok(info)
        }
    }
    fn activate(&mut self, config: Config) -> Result<Box<dyn FormatProcessor>, String> {
        if self.active {
            return Err("VST3 instance already active".into());
        }
        let info = self.metadata()?;
        if !fits(&info, config) {
            return Err("VST3 port configuration mismatch".into());
        }
        unsafe {
            ok(self.processor.canProcessSampleSize(0))?;
            let mut inputs = vec![0; info.inputs.len()];
            let mut outputs = vec![0; info.outputs.len()];
            for (direction, ports) in [(0, &mut inputs), (1, &mut outputs)] {
                for (index, arrangement) in ports.iter_mut().enumerate() {
                    ok(self
                        .processor
                        .getBusArrangement(direction, index as i32, arrangement))?;
                }
            }
            ok(self.processor.setBusArrangements(
                inputs.as_mut_ptr(),
                inputs.len() as i32,
                outputs.as_mut_ptr(),
                outputs.len() as i32,
            ))?;
            for (media, direction, count) in [
                (0, 0, info.inputs.len()),
                (0, 1, info.outputs.len()),
                (1, 0, info.note_inputs.len()),
                (1, 1, info.note_outputs.len()),
            ] {
                for index in 0..count {
                    ok(self
                        .component
                        .activateBus(media, direction, index as i32, 1))?;
                }
            }
            let mut setup = ProcessSetup {
                processMode: 0,
                symbolicSampleSize: 0,
                maxSamplesPerBlock: FRAMES as i32,
                sampleRate: config.sample_rate,
            };
            ok(self.processor.setupProcessing(&mut setup))?;
            ok(self.component.setActive(1))?;
            self.active = true;
        }
        let mut midi = vec![u32::MAX; info.note_inputs.len() * 16 * 130];
        if let Some(mapping) = self.controller.cast::<IMidiMapping>() {
            for port in 0..info.note_inputs.len() {
                for channel in 0..16 {
                    for cc in 0..130 {
                        let mut id = u32::MAX;
                        unsafe {
                            if mapping.getMidiControllerAssignment(
                                port as i32,
                                channel as i16,
                                cc as i16,
                                &mut id,
                            ) == kResultOk
                            {
                                midi[(port * 16 + channel) * 130 + cc] = id;
                            }
                        }
                    }
                }
            }
        }
        Ok(Box::new(Processor {
            edits: self.edits.take().ok_or("VST3 already activated")?,
            editor_parameters: EventList::default(),
            midi,
            note_inputs: info.note_inputs.len(),
            note_outputs: info.note_outputs.len(),
            processor: self.processor.clone(),
            shared: Arc::clone(&self.shared),
            config,
            inputs: info.inputs.iter().map(|p| p.channels as usize).collect(),
            outputs: info.outputs.iter().map(|p| p.channels as usize).collect(),
            input_params: ComWrapper::new(Changes::new()),
            output_params: ComWrapper::new(Changes::new()),
            input_events: ComWrapper::new(Events::new()),
            output_events: ComWrapper::new(Events::new()),
            running: false,
        }))
    }
    fn deactivate(&mut self, processor: Box<dyn Any + Send>) -> Result<(), String> {
        let processor = processor
            .downcast::<Processor>()
            .map_err(|_| "wrong VST3 processor")?;
        self.edits = Some(processor.edits);
        unsafe {
            ok(self.component.setActive(0))?;
        }
        self.active = false;
        Ok(())
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.main_callback();
        let a = ComWrapper::new(Stream::new(vec![]));
        let b = ComWrapper::new(Stream::new(vec![]));
        unsafe {
            ok(self
                .component
                .getState(a.to_com_ptr::<IBStream>().unwrap().as_ptr()))?;
            let result = self
                .controller
                .getState(b.to_com_ptr::<IBStream>().unwrap().as_ptr());
            if result != kNotImplemented {
                ok(result)?;
            }
        }
        if a.failed.get() || b.failed.get() {
            return Err("VST3 state stream capacity exceeded".into());
        }
        let a = a.bytes.borrow();
        let b = b.bytes.borrow();
        if a.len() + b.len() + 8 > MAX_STATE_BYTES {
            return Err("VST3 state capacity exceeded".into());
        }
        let mut bytes = Vec::with_capacity(a.len() + b.len() + 8);
        bytes.extend_from_slice(b"VST3");
        bytes.extend_from_slice(&(a.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&a);
        bytes.extend_from_slice(&b);
        Ok(bytes)
    }
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() < 8 || bytes.len() > MAX_STATE_BYTES || &bytes[..4] != b"VST3" {
            return Err("invalid VST3 state image".into());
        }
        let length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if length > bytes.len() - 8 {
            return Err("invalid VST3 state image".into());
        }
        let a = ComWrapper::new(Stream::new(bytes[8..8 + length].to_vec()));
        unsafe {
            ok(self
                .component
                .setState(a.to_com_ptr::<IBStream>().unwrap().as_ptr()))?;
            a.position.set(0);
            let result = self
                .controller
                .setComponentState(a.to_com_ptr::<IBStream>().unwrap().as_ptr());
            if result != kNotImplemented {
                ok(result)?;
            }
            if bytes.len() > 8 + length {
                let b = ComWrapper::new(Stream::new(bytes[8 + length..].to_vec()));
                ok(self
                    .controller
                    .setState(b.to_com_ptr::<IBStream>().unwrap().as_ptr()))?;
            }
        }
        self.refresh_values();
        Ok(())
    }
    fn parameter(&mut self, id: u32) -> Result<f64, String> {
        self.shared
            .values
            .iter()
            .find(|p| p.0 == id)
            .map(|p| f64::from_bits(p.1.load(Ordering::Acquire)))
            .ok_or("unknown VST3 parameter".into())
    }
    fn value_text(&mut self, id: u32, value: f64) -> Result<String, String> {
        let mut buffer = [0u16; 128];
        unsafe {
            ok(self
                .controller
                .getParamStringByValue(id, value, &mut buffer))?;
        }
        Ok(wide(&buffer))
    }
    fn text_value(&mut self, id: u32, text: &str) -> Result<f64, String> {
        let mut buffer = [0u16; 128];
        let chars: Vec<_> = text.encode_utf16().collect();
        if chars.len() >= buffer.len() {
            return Err("parameter text too long".into());
        }
        buffer[..chars.len()].copy_from_slice(&chars);
        let mut value = 0.0;
        unsafe {
            ok(self
                .controller
                .getParamValueByString(id, buffer.as_mut_ptr(), &mut value))?;
        }
        if !value.is_finite() {
            return Err("non-finite parameter value".into());
        }
        Ok(value)
    }
    fn editor(&mut self, command: EditorCommand) -> Result<EditorState, String> {
        if matches!(command, EditorCommand::Close) {
            let view = self.windows.borrow_mut().view.take();
            if let Some(view) = view {
                // SAFETY: view is attached to our retained parent on this main thread.
                unsafe {
                    let _ = view.removed();
                    let _ = view.setFrame(ptr::null_mut());
                }
            }
            self.windows.borrow_mut().window = None;
            return Ok(EditorState::default());
        }
        if matches!(command, EditorCommand::Open) && self.windows.borrow().view.is_none() {
            // SAFETY: controller and editor lifecycle remain on main.
            unsafe {
                let view = ComPtr::from_raw(self.controller.createView(c"editor".as_ptr()))
                    .ok_or("plugin has no native editor")?;
                ok(view.isPlatformTypeSupported(c"X11EmbedWindowID".as_ptr()))?;
                let mut rect: ViewRect = zero();
                ok(view.getSize(&mut rect))?;
                let mut window = Window::new(
                    &self.info.name,
                    (rect.right - rect.left) as u32,
                    (rect.bottom - rect.top) as u32,
                )?;
                let resizable = view.canResize() == kResultOk;
                window.resize_hints(resizable, resizable);
                let id = window.id;
                {
                    let mut w = self.windows.borrow_mut();
                    w.window = Some(window);
                    w.view = Some(view.clone());
                }
                let result = (|| {
                    ok(view.setFrame(
                        self.handler
                            .as_ref()
                            .unwrap()
                            .to_com_ptr::<IPlugFrame>()
                            .unwrap()
                            .as_ptr(),
                    ))?;
                    ok(view.attached(id as usize as *mut c_void, c"X11EmbedWindowID".as_ptr()))?;
                    ok(view.onSize(&mut rect))?;
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    let _ = self.editor(EditorCommand::Close);
                    return Err(error);
                }
            }
        }
        if matches!(command, EditorCommand::Open)
            && let Some(w) = &self.windows.borrow().window
        {
            w.show();
        }
        if let EditorCommand::Resize { width, height } = command {
            if !EditorState::valid_size(width, height) {
                return Err("invalid plugin editor size".into());
            }
            let view = self
                .windows
                .borrow()
                .view
                .clone()
                .ok_or("plugin editor is closed")?;
            // SAFETY: main-thread view, stack rect remains live throughout these calls.
            unsafe {
                ok(view.canResize()).map_err(|_| "plugin editor does not support host resizing")?;
                let mut rect = ViewRect {
                    left: 0,
                    top: 0,
                    right: width as i32,
                    bottom: height as i32,
                };
                ok(view.checkSizeConstraint(&mut rect))?;
                self.windows
                    .borrow_mut()
                    .window
                    .as_mut()
                    .ok_or("plugin editor is closed")?
                    .resize(
                        (rect.right - rect.left) as u32,
                        (rect.bottom - rect.top) as u32,
                    )?;
                ok(view.onSize(&mut rect))?;
            }
        }
        Ok(self
            .windows
            .borrow()
            .window
            .as_ref()
            .map(|w| w.state())
            .unwrap_or_default())
    }
    fn main_callback(&mut self) {
        let timers: Vec<_> = self
            .windows
            .borrow_mut()
            .timers
            .iter_mut()
            .filter_map(|t| t.due().then(|| t.id.clone()))
            .collect();
        for h in timers {
            if self
                .windows
                .borrow()
                .timers
                .iter()
                .any(|t| t.id.as_ptr() == h.as_ptr())
            {
                // SAFETY: retained COM timer, called on main without reactor borrows.
                unsafe {
                    h.onTimer();
                }
            }
        }
        let fds = self.windows.borrow().fds.clone();
        for (fd, h) in fds {
            if editor::fd_ready(fd, libc::POLLIN) != 0
                && self
                    .windows
                    .borrow()
                    .fds
                    .iter()
                    .any(|p| p.0 == fd && p.1.as_ptr() == h.as_ptr())
            {
                // SAFETY: retained COM handler, called on main without reactor borrows.
                unsafe {
                    h.onFDIsSet(fd);
                }
            }
        }
        let events = self
            .windows
            .borrow_mut()
            .window
            .as_mut()
            .map(|w| w.events())
            .unwrap_or_default();
        for event in events {
            match event {
                WindowEvent::Close => {
                    let _ = self.editor(EditorCommand::Close);
                }
                WindowEvent::Resize(width, height) => {
                    let _ = self.editor(EditorCommand::Resize { width, height });
                }
            }
        }
        if self.shared.values_changed.swap(false, Ordering::AcqRel) {
            self.refresh_values();
        }
        for (id, value) in &self.shared.values {
            unsafe {
                let _ = self
                    .controller
                    .setParamNormalized(*id, f64::from_bits(value.load(Ordering::Acquire)));
            }
        }
    }
    fn restart_requested(&self) -> bool {
        self.shared.restart.swap(false, Ordering::AcqRel)
    }
}
pub(crate) fn scan(path: &Path, offset: usize) -> Result<Vec<ScanEntry>, String> {
    let module = module(path)?;
    let classes = classes(&module)?;
    drop(module);
    let mut rows = vec![];
    for class in classes.into_iter().skip(offset).take(4) {
        let result = (|| {
            let mut instance = load(path, &identity(class.cid))?;
            let info = instance.metadata()?;
            let config = Config {
                generation: 1,
                sample_rate: 48000.0,
                sample_origin: 0,
                inputs: info.inputs.iter().map(|p| p.channels).sum(),
                outputs: info.outputs.iter().map(|p| p.channels).sum(),
                depth: 4,
                reserved: 0,
            };
            let processor = instance.activate(config)?;
            let metadata = instance.metadata()?;
            instance.deactivate(processor.stop())?;
            Ok::<_, String>(metadata)
        })();
        rows.push(match result {
            Ok(plugin) => ScanEntry::Plugin {
                plugin: Box::new(plugin),
            },
            Err(error) => ScanEntry::Error {
                path: path.to_string_lossy().into_owned(),
                error,
            },
        });
    }
    Ok(rows)
}

struct Queue {
    id: Cell<u32>,
    points: RefCell<Vec<(i32, f64)>>,
    failed: Cell<bool>,
}
impl Class for Queue {
    type Interfaces = (IParamValueQueue,);
}
impl IParamValueQueueTrait for Queue {
    unsafe fn getParameterId(&self) -> u32 {
        self.id.get()
    }
    unsafe fn getPointCount(&self) -> i32 {
        self.points.borrow().len() as i32
    }
    unsafe fn getPoint(&self, index: i32, offset: *mut i32, value: *mut f64) -> tresult {
        if offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        if let Some(&(o, v)) = self.points.borrow().get(index as usize) {
            unsafe {
                *offset = o;
                *value = v;
            }
            kResultOk
        } else {
            kInvalidArgument
        }
    }
    unsafe fn addPoint(&self, offset: i32, value: f64, index: *mut i32) -> tresult {
        let mut points = self.points.borrow_mut();
        if points.len() == EVENTS
            || !value.is_finite()
            || !(0.0..=1.0).contains(&value)
            || !(0..FRAMES as i32).contains(&offset)
        {
            self.failed.set(true);
            return kResultFalse;
        }
        if !index.is_null() {
            unsafe {
                *index = points.len() as i32;
            }
        }
        points.push((offset, value));
        kResultOk
    }
}
struct Changes {
    queues: Vec<ComWrapper<Queue>>,
    count: Cell<usize>,
    failed: Cell<bool>,
}
impl Changes {
    fn new() -> Self {
        Self {
            queues: (0..EVENTS)
                .map(|_| {
                    ComWrapper::new(Queue {
                        id: Cell::new(0),
                        points: RefCell::new(Vec::with_capacity(EVENTS)),
                        failed: Cell::new(false),
                    })
                })
                .collect(),
            count: Cell::new(0),
            failed: Cell::new(false),
        }
    }
    fn clear(&self) {
        self.count.set(0);
        self.failed.set(false);
        for q in &self.queues {
            q.points.borrow_mut().clear();
            q.failed.set(false);
        }
    }
    fn queue(&self, id: u32) -> Option<&ComWrapper<Queue>> {
        let count = self.count.get();
        if let Some(q) = self.queues[..count].iter().find(|q| q.id.get() == id) {
            return Some(q);
        }
        let Some(q) = self.queues.get(count) else {
            self.failed.set(true);
            return None;
        };
        q.id.set(id);
        self.count.set(count + 1);
        Some(q)
    }
}
impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}
impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> i32 {
        self.count.get() as i32
    }
    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        self.queues
            .get(index as usize)
            .filter(|_| (index as usize) < self.count.get())
            .map_or(ptr::null_mut(), |q| {
                q.as_com_ref::<IParamValueQueue>().unwrap().as_ptr()
            })
    }
    unsafe fn addParameterData(&self, id: *const u32, index: *mut i32) -> *mut IParamValueQueue {
        if id.is_null() {
            return ptr::null_mut();
        }
        let Some(q) = self.queue(unsafe { *id }) else {
            return ptr::null_mut();
        };
        if !index.is_null() {
            unsafe {
                *index = self.queues[..self.count.get()]
                    .iter()
                    .position(|p| p.id.get() == *id)
                    .unwrap() as i32;
            }
        }
        q.as_com_ref::<IParamValueQueue>().unwrap().as_ptr()
    }
}
struct Events {
    events: RefCell<Vec<Event>>,
    failed: Cell<bool>,
}
impl Events {
    fn new() -> Self {
        Self {
            events: RefCell::new(Vec::with_capacity(EVENTS)),
            failed: Cell::new(false),
        }
    }
    fn clear(&self) {
        self.events.borrow_mut().clear();
        self.failed.set(false);
    }
}
impl Class for Events {
    type Interfaces = (IEventList,);
}
impl IEventListTrait for Events {
    unsafe fn getEventCount(&self) -> i32 {
        self.events.borrow().len() as i32
    }
    unsafe fn getEvent(&self, index: i32, event: *mut Event) -> tresult {
        if event.is_null() {
            return kInvalidArgument;
        }
        if let Some(e) = self.events.borrow().get(index as usize) {
            unsafe {
                *event = *e;
            }
            kResultOk
        } else {
            kInvalidArgument
        }
    }
    unsafe fn addEvent(&self, event: *mut Event) -> tresult {
        if event.is_null() {
            return kInvalidArgument;
        }
        let mut events = self.events.borrow_mut();
        if events.len() == EVENTS {
            self.failed.set(true);
            return kResultFalse;
        }
        // Only inline events are retained; SDK data/text pointers have plugin lifetime.
        let event = unsafe { *event };
        if !matches!(event.r#type, 0 | 1 | 3 | 4 | 65535) {
            self.failed.set(true);
            return kResultFalse;
        }
        events.push(event);
        kResultOk
    }
}
struct Processor {
    edits: rtrb::Consumer<WireEvent>,
    editor_parameters: EventList,
    midi: Vec<u32>,
    note_inputs: usize,
    note_outputs: usize,
    processor: ComPtr<IAudioProcessor>,
    shared: Arc<Shared>,
    config: Config,
    inputs: Vec<usize>,
    outputs: Vec<usize>,
    input_params: ComWrapper<Changes>,
    output_params: ComWrapper<Changes>,
    input_events: ComWrapper<Events>,
    output_events: ComWrapper<Events>,
    running: bool,
}
// SAFETY: processor and its COM event objects transfer exclusively to one audio
// thread, return stopped to main, and never call the edit controller on audio.
unsafe impl Send for Processor {}
impl FormatProcessor for Processor {
    fn start(&mut self) -> Result<(), String> {
        if !self.running {
            unsafe {
                ok(self.processor.setProcessing(1))?;
            }
            self.running = true;
        }
        Ok(())
    }
    fn pause(&mut self) {
        if self.running {
            unsafe {
                let _ = self.processor.setProcessing(0);
            }
            self.running = false;
        }
    }
    fn stop(mut self: Box<Self>) -> Box<dyn Any + Send> {
        self.pause();
        self
    }
    fn editor_events(&mut self, result: &mut EventList) -> Result<(), String> {
        for _ in 0..EVENTS {
            let Ok(event) = self.edits.pop() else {
                break;
            };
            if !result.push(event) {
                return Err("VST3 callback capacity exceeded".into());
            }
            if event.kind == EventKind::Parameter as u32 {
                if let Some(current) = self.editor_parameters.values
                    [..self.editor_parameters.count as usize]
                    .iter_mut()
                    .find(|e| e.id == event.id)
                {
                    *current = event;
                } else if !self.editor_parameters.push(event) {
                    return Err("VST3 editor parameter capacity exceeded".into());
                }
            }
        }
        Ok(())
    }
    fn process(
        &mut self,
        span: Span,
        steady: u64,
        input: &[f32],
        output: &mut [f32],
        events: &EventList,
        result: &mut EventList,
    ) -> Result<(), String> {
        let offset = span.offset as usize;
        let frames = span.frames as usize;
        self.input_params.clear();
        self.output_params.clear();
        self.input_events.clear();
        self.output_events.clear();
        for event in &self.editor_parameters.values[..self.editor_parameters.count as usize] {
            let queue = self
                .input_params
                .queue(event.id)
                .ok_or("VST3 callback capacity exceeded")?;
            unsafe {
                ok(queue.addPoint(0, event.value, ptr::null_mut()))?;
            }
        }
        self.editor_parameters.count = 0;
        for event in &events.values[..events.count as usize] {
            if event.offset >= span.frames {
                return Err("invalid VST3 input offset".into());
            }
            if event.kind == EventKind::Parameter as u32 {
                let value = self
                    .shared
                    .values
                    .iter()
                    .find(|p| p.0 == event.id)
                    .ok_or("unknown VST3 parameter")?;
                let q = self
                    .input_params
                    .queue(event.id)
                    .ok_or("VST3 parameter queue capacity exceeded")?;
                unsafe {
                    if q.addPoint(event.offset as i32, event.value, ptr::null_mut()) != kResultOk {
                        return Err("invalid VST3 parameter event".into());
                    }
                }
                value.1.store(event.value.to_bits(), Ordering::Release);
            } else {
                if event.port < 0 || event.port as usize >= self.note_inputs {
                    return Err("unknown VST3 event port".into());
                }
                let mut event = *event;
                if event.kind == EventKind::Midi as u32 {
                    let status = event.id as u8;
                    let first = ((event.id >> 8) & 127) as u8;
                    let second = ((event.id >> 16) & 127) as u8;
                    event.channel = i16::from(status & 15);
                    event.key = i16::from(first);
                    event.note_id = -1;
                    match status & 0xf0 {
                        0x80 | 0x90 => {
                            event.kind = if status & 0xf0 == 0x90 && second > 0 {
                                EventKind::NoteOn as u32
                            } else {
                                EventKind::NoteOff as u32
                            };
                            event.value = f64::from(second) / 127.0;
                        }
                        0xa0 => {
                            event.kind = EventKind::Expression as u32;
                            event.id = 6;
                            event.value = f64::from(second) / 127.0;
                        }
                        0xb0 | 0xd0 | 0xe0 => {
                            let (cc, value) = match status & 0xf0 {
                                0xb0 => (first as usize, f64::from(second) / 127.0),
                                0xd0 => (128, f64::from(first) / 127.0),
                                _ => (
                                    129,
                                    f64::from(u16::from(first) | (u16::from(second) << 7))
                                        / 16383.0,
                                ),
                            };
                            let id = self.midi
                                [(event.port as usize * 16 + event.channel as usize) * 130 + cc];
                            if id == u32::MAX {
                                continue;
                            }
                            let target = self
                                .shared
                                .values
                                .iter()
                                .find(|p| p.0 == id)
                                .ok_or("unknown VST3 MIDI parameter")?;
                            target.1.store(value.to_bits(), Ordering::Release);
                            let q = self
                                .input_params
                                .queue(id)
                                .ok_or("VST3 MIDI parameter capacity exceeded")?;
                            unsafe {
                                if q.addPoint(event.offset as i32, value, ptr::null_mut())
                                    != kResultOk
                                {
                                    return Err("VST3 MIDI parameter capacity exceeded".into());
                                }
                            }
                            continue;
                        }
                        _ => return Err("unsupported VST3 MIDI message".into()),
                    }
                }
                let mut native: Event = zero();
                native.busIndex = i32::from(event.port.max(0));
                native.sampleOffset = event.offset as i32;
                match event.kind {
                    k if k == EventKind::NoteOn as u32 => {
                        native.r#type = 0;
                        native.__field0.noteOn = NoteOnEvent {
                            channel: event.channel.max(0),
                            pitch: event.key,
                            tuning: 0.0,
                            velocity: event.value as f32,
                            length: 0,
                            noteId: event.note_id,
                        };
                    }
                    k if k == EventKind::NoteOff as u32 || k == EventKind::NoteChoke as u32 => {
                        native.r#type = 1;
                        native.__field0.noteOff = NoteOffEvent {
                            channel: event.channel.max(0),
                            pitch: event.key,
                            velocity: event.value as f32,
                            noteId: event.note_id,
                            tuning: 0.0,
                        };
                    }
                    k if k == EventKind::Expression as u32 => {
                        if event.id == 6 {
                            native.r#type = 3;
                            native.__field0.polyPressure = PolyPressureEvent {
                                channel: event.channel,
                                pitch: event.key,
                                pressure: event.value as f32,
                                noteId: event.note_id,
                            };
                        } else {
                            let value = match event.id {
                                0 => event.value / 4.0,
                                2 => event.value / 240.0 + 0.5,
                                1 | 3 | 4 | 5 => event.value,
                                _ => return Err("unsupported VST3 expression".into()),
                            };
                            if !(0.0..=1.0).contains(&value) {
                                return Err("VST3 expression outside native range".into());
                            }
                            native.r#type = 4;
                            native.__field0.noteExpressionValue = NoteExpressionValueEvent {
                                typeId: event.id,
                                noteId: event.note_id,
                                value,
                            };
                        }
                    }
                    _ => return Err("unsupported VST3 event dialect".into()),
                }
                unsafe {
                    if self.input_events.addEvent(&mut native) != kResultOk {
                        return Err("VST3 event capacity exceeded".into());
                    }
                }
            }
        }
        let mut in_ptrs = [ptr::null_mut(); clausters_core::plugin_shm::CHANNELS as usize];
        let mut out_ptrs = [ptr::null_mut(); clausters_core::plugin_shm::CHANNELS as usize];
        for (channel, p) in in_ptrs
            .iter_mut()
            .enumerate()
            .take(self.config.inputs as usize)
        {
            *p = input[channel * FRAMES + offset..].as_ptr() as *mut f32;
        }
        for (channel, p) in out_ptrs
            .iter_mut()
            .enumerate()
            .take(self.config.outputs as usize)
        {
            *p = output[channel * FRAMES + offset..].as_mut_ptr();
        }
        let mut in_buses: [AudioBusBuffers; MAX_PORTS] = zero();
        let mut out_buses: [AudioBusBuffers; MAX_PORTS] = zero();
        for (ports, pointers, buses) in [
            (&self.inputs, &mut in_ptrs, &mut in_buses),
            (&self.outputs, &mut out_ptrs, &mut out_buses),
        ] {
            let mut first = 0;
            for (index, &channels) in ports.iter().enumerate() {
                buses[index].numChannels = channels as i32;
                buses[index].__field0.channelBuffers32 = pointers[first..].as_mut_ptr();
                first += channels;
            }
        }
        let mut context: ProcessContext = zero();
        context.sampleRate = self.config.sample_rate;
        context.projectTimeSamples = span.position;
        context.continousTimeSamples = steady as i64;
        context.tempo = span.tempo;
        let (beats, bar_start, _) = musical_position(&span, self.config.sample_rate);
        context.projectTimeMusic = beats;
        context.barPositionMusic = bar_start;
        context.timeSigNumerator = BEATS_PER_BAR as i32;
        context.timeSigDenominator = 4;
        context.state = ProcessContext_::StatesAndFlags_::kTempoValid
            | ProcessContext_::StatesAndFlags_::kContTimeValid
            | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
            | ProcessContext_::StatesAndFlags_::kBarPositionValid
            | ProcessContext_::StatesAndFlags_::kTimeSigValid;
        if span.playing != 0 {
            context.state |= ProcessContext_::StatesAndFlags_::kPlaying;
        }
        let mut data = ProcessData {
            processMode: 0,
            symbolicSampleSize: 0,
            numSamples: frames as i32,
            numInputs: self.inputs.len() as i32,
            numOutputs: self.outputs.len() as i32,
            inputs: in_buses.as_mut_ptr(),
            outputs: out_buses.as_mut_ptr(),
            inputParameterChanges: self
                .input_params
                .as_com_ref::<IParameterChanges>()
                .unwrap()
                .as_ptr(),
            outputParameterChanges: self
                .output_params
                .as_com_ref::<IParameterChanges>()
                .unwrap()
                .as_ptr(),
            inputEvents: self
                .input_events
                .as_com_ref::<IEventList>()
                .unwrap()
                .as_ptr(),
            outputEvents: self
                .output_events
                .as_com_ref::<IEventList>()
                .unwrap()
                .as_ptr(),
            processContext: &mut context,
        };
        unsafe {
            ok(self.processor.process(&mut data))?;
        }
        if self.output_events.failed.get()
            || self.output_params.failed.get()
            || self.output_params.queues.iter().any(|q| q.failed.get())
        {
            return Err("VST3 output event capacity exceeded".into());
        }
        let mut collected = EventList::default();
        for q in &self.output_params.queues[..self.output_params.count.get()] {
            for &(sample, value) in q.points.borrow().iter() {
                if sample < 0 || sample >= frames as i32 {
                    return Err("invalid VST3 output offset".into());
                }
                let target = self
                    .shared
                    .values
                    .iter()
                    .find(|p| p.0 == q.id.get())
                    .ok_or("unknown VST3 output parameter")?;
                target.1.store(value.to_bits(), Ordering::Release);
                if collected.count as usize == EVENTS {
                    return Err("VST3 output capacity exceeded".into());
                }
                collected.values[collected.count as usize] = WireEvent {
                    offset: span.offset + sample as u32,
                    kind: EventKind::Parameter as u32,
                    id: q.id.get(),
                    value,
                    note_id: -1,
                    port: -1,
                    channel: -1,
                    key: -1,
                    ..WireEvent::default()
                };
                collected.count += 1;
            }
        }
        for native in self.output_events.events.borrow().iter() {
            if native.sampleOffset < 0 || native.sampleOffset >= frames as i32 {
                return Err("invalid VST3 output offset".into());
            }
            if native.busIndex < 0 || native.busIndex as usize >= self.note_outputs {
                return Err("unknown VST3 output event port".into());
            }
            let mut wire = WireEvent {
                offset: span.offset + native.sampleOffset as u32,
                port: native.busIndex as i16,
                note_id: -1,
                channel: -1,
                key: -1,
                ..WireEvent::default()
            };
            unsafe {
                match native.r#type {
                    0 => {
                        let e = native.__field0.noteOn;
                        wire.kind = EventKind::NoteOn as u32;
                        wire.note_id = e.noteId;
                        wire.channel = e.channel;
                        wire.key = e.pitch;
                        wire.value = f64::from(e.velocity);
                    }
                    1 => {
                        let e = native.__field0.noteOff;
                        wire.kind = EventKind::NoteOff as u32;
                        wire.note_id = e.noteId;
                        wire.channel = e.channel;
                        wire.key = e.pitch;
                        wire.value = f64::from(e.velocity);
                    }
                    4 => {
                        let e = native.__field0.noteExpressionValue;
                        wire.kind = EventKind::Expression as u32;
                        wire.id = e.typeId;
                        wire.note_id = e.noteId;
                        wire.value = match e.typeId {
                            0 => e.value * 4.0,
                            2 => e.value * 240.0 - 120.0,
                            1 | 3 | 4 | 5 => e.value,
                            _ => return Err("unsupported VST3 output expression".into()),
                        };
                    }
                    3 => {
                        let e = native.__field0.polyPressure;
                        wire.kind = EventKind::Expression as u32;
                        wire.id = 6;
                        wire.note_id = e.noteId;
                        wire.channel = e.channel;
                        wire.key = e.pitch;
                        wire.value = f64::from(e.pressure);
                    }
                    65535 => {
                        let e = native.__field0.midiCCOut;
                        wire.kind = EventKind::Midi as u32;
                        wire.channel = e.channel as i16;
                        let (status, first, second) = match e.controlNumber {
                            0..=127 => (0xb0, e.controlNumber, e.value as u8),
                            128 => (0xd0, e.value as u8, 0),
                            129 => (0xe0, e.value as u8, e.value2 as u8),
                            _ => return Err("unsupported VST3 MIDI output".into()),
                        };
                        wire.id = u32::from(status | e.channel as u8)
                            | u32::from(first) << 8
                            | u32::from(second) << 16;
                    }
                    _ => return Err("unsupported VST3 output event".into()),
                }
            }
            if collected.count as usize == EVENTS {
                return Err("VST3 output capacity exceeded".into());
            }
            collected.values[collected.count as usize] = wire;
            collected.count += 1;
        }
        // Stable insertion sort keeps begin/edit/end order at equal offsets,
        // without the allocation allowed by slice's stable sort.
        for index in 1..collected.count as usize {
            let mut current = index;
            while current > 0
                && collected.values[current - 1].offset > collected.values[current].offset
            {
                collected.values.swap(current - 1, current);
                current -= 1;
            }
        }
        for &event in &collected.values[..collected.count as usize] {
            if !result.push(event) {
                return Err("VST3 output capacity exceeded".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streams_and_com_event_containers_reject_overflow() {
        let stream = Stream::new(vec![]);
        unsafe {
            assert_eq!(
                stream.seek(MAX_STATE_BYTES as i64, 0, ptr::null_mut()),
                kResultOk
            );
            assert_eq!(
                stream.write([1u8].as_ptr().cast_mut().cast(), 1, ptr::null_mut()),
                kResultFalse
            );
            assert!(stream.failed.get());
            assert_eq!(stream.seek(-1, 0, ptr::null_mut()), kInvalidArgument);
            let queues = Changes::new();
            for id in 0..EVENTS {
                let q = queues.queue(id as u32).unwrap();
                assert_eq!(q.addPoint(0, 0.5, ptr::null_mut()), kResultOk);
            }
            assert!(queues.queue(EVENTS as u32).is_none());
            assert!(queues.failed.get());
            queues.clear();
            assert_eq!(queues.count.get(), 0);
            let events = Events::new();
            let mut event: Event = zero();
            event.r#type = 0;
            for _ in 0..EVENTS {
                assert_eq!(events.addEvent(&mut event), kResultOk);
            }
            assert_eq!(events.addEvent(&mut event), kResultFalse);
            assert!(events.failed.get());
            events.clear();
            assert_eq!(events.getEventCount(), 0);
        }
    }
}
