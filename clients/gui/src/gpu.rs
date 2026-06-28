//! The wgpu device/surface bring-up, shared by the native window harness
//! ([`crate::native`]), the GUI host's windowed front ([`crate::host::gui`]) and
//! the browser entry point ([`crate::host::web`]).
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
    /// Returns an error rather than panicking when no GPU is available — most
    /// importantly in a browser whose WebGPU is not enabled (e.g. Linux Chrome
    /// without the WebGPU/Vulkan flags), where `request_adapter` finds none — so
    /// the front can surface a clear message instead of aborting.
    pub(crate) async fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
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
            .map_err(|e| {
                format!(
                    "no suitable GPU adapter ({e}); the browser may not have WebGPU enabled \
                     (on Linux Chrome, enable chrome://flags/#enable-unsafe-webgpu and Vulkan, \
                     or use a WebGPU-capable browser)"
                )
            })?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("clausters-gui device"),
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
