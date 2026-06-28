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
    pub(crate) async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("clausters-gui device"),
                ..Default::default()
            })
            .await
            .expect("request device");
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by adapter");
        surface.configure(&device, &config);
        Self {
            window,
            surface,
            device,
            queue,
            config,
        }
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }
}
