//! The wgpu device/surface bring-up, shared by the native window harness
//! ([`crate::native`]), the GUI host's windowed front ([`crate::host::gui`]) and
//! the browser entry point (`crate::host::web`).
//!
//! It is **platform-agnostic**: `wgpu` and `winit` both compile for `wasm32`
//! (the WebGPU backend over an HTML `<canvas>`), so the device/surface setup
//! lives once. The only platform difference is *when* it is awaited — natively
//! `pollster::block_on(Gpu::new(window))` in `resumed`, on the web an async task
//! that resolves the adapter/device without blocking the browser's main thread.

use std::sync::Arc;

use winit::window::Window;

/// The window plus its wgpu surface, device, queue and current configuration.
pub(crate) struct Gpu {
    pub(crate) window: Arc<Window>,
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    /// Requests an adapter/device for `window`'s surface and configures it to the
    /// window's current size. `async` so the web path can await it (the native
    /// path blocks on it); the adapter/device requests are the only await points.
    ///
    /// Returns an error rather than panicking when no GPU is available, so the
    /// front can surface a clear message instead of aborting. On the web this is
    /// rare: [`new_instance`] prefers WebGPU where the browser truly supports it
    /// and otherwise falls back to **WebGL2**, which nearly every browser has —
    /// so a Linux/older-Android browser whose WebGPU is disabled still renders.
    pub(crate) async fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = new_instance().await;
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("cannot create the GPU surface: {e}"))?;
        // No power preference (the default): more permissive than insisting on a
        // discrete GPU, which matters on integrated-only and browser adapters.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| format!("no suitable GPU adapter ({e}); {NO_ADAPTER_HINT}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("clausters-gui device"),
                // Native keeps wgpu's full defaults; the web caps to the WebGL2
                // downlevel set so the device also comes up on a WebGL2 adapter.
                required_limits: device_limits(&adapter),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("cannot create the GPU device: {e}"))?;
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| "the surface is unsupported by this GPU adapter".to_string())?;
        surface.configure(&device, &config);
        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
        })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }
}

/// Builds the wgpu instance for the platform.
///
/// On the web it enables **both** the WebGPU and the WebGL2 backends and decides
/// at instance creation through `new_instance_with_webgpu_detection`: it keeps
/// WebGPU only when the browser can actually create a WebGPU adapter (the helper
/// probes for one, not just for `navigator.gpu` — Linux Chrome exposes that
/// property yet cannot create an adapter), and otherwise drops to WebGL2. That
/// runtime fall-through is what gives the web host reach on browsers where
/// WebGPU is disabled. Native uses the default backends unchanged.
#[cfg(target_arch = "wasm32")]
async fn new_instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
    wgpu::util::new_instance_with_webgpu_detection(desc).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn new_instance() -> wgpu::Instance {
    wgpu::Instance::default()
}

/// The device limits to request. The web caps to the WebGL2 downlevel set so
/// `request_device` also succeeds on a WebGL2 adapter, while `using_resolution`
/// lifts the texture-size limits back to whatever the adapter actually reports
/// (a long spectrogram texture needs the real maximum, not the 2048 floor). On a
/// WebGPU adapter this stays well within support. Native keeps wgpu's defaults.
#[cfg(target_arch = "wasm32")]
fn device_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
}

#[cfg(not(target_arch = "wasm32"))]
fn device_limits(_adapter: &wgpu::Adapter) -> wgpu::Limits {
    wgpu::Limits::default()
}

/// The hint appended to the "no GPU adapter" error, tailored per platform.
#[cfg(target_arch = "wasm32")]
const NO_ADAPTER_HINT: &str = "on the web this means neither WebGPU nor WebGL2 is available; \
     almost every browser supports WebGL2, so check that hardware acceleration / WebGL is enabled, \
     or try another browser";
#[cfg(not(target_arch = "wasm32"))]
const NO_ADAPTER_HINT: &str = "no Vulkan/Metal/DX12 device was found; check the GPU drivers";
