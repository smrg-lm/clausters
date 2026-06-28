//! Rendering one window's widget tree into its wgpu surface — the shared frame
//! path, agnostic of platform and of how the host is driven.
//!
//! This is the code the milestone calls "isolate the surface/GPU/loop port":
//! both fronts feed the **same** [`render`] one tree plus its per-window GPU
//! resources, so the browser is pixel-faithful to the desktop by construction,
//! not by a parallel renderer. The native windowed front ([`super::gui`]) calls
//! it with live inputs (the shared-memory bus source, scope histories, the node
//! tree, the held-button highlight); the browser entry point ([`super::web`])
//! calls it with empty inputs (G12 has no transport yet, so meters read zero and
//! there is no node tree). It builds the flat-geometry [`Mesh`] from the placed
//! widgets ([`super::layout`] + [`super::paint`]/[`super::font`]), uploads the
//! heavy `waveform`/`canvas` views, and draws the whole frame in one pass.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::gpu::Gpu;
use crate::view::TimelineView;
use crate::viewport::View;
use crate::waveform::{WaveformData, WaveformView};

use super::canvas::{self, CanvasView};
use super::layout::{self, Rect};
use super::nodetree::{self, NodeTree};
use super::paint::{Color, Mesh, Painter};
use super::widget::{Widget, WidgetKind};
use super::{BusSource, controls, meters, plot};

/// The window's clear color (the dark chrome backdrop).
pub(crate) const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.05,
    g: 0.05,
    b: 0.07,
    a: 1.0,
};
const PANEL_COLOR: Color = [0.10, 0.11, 0.14, 0.55];
const LABEL_COLOR: Color = [0.85, 0.87, 0.90, 1.0];
const LABEL_SCALE: f32 = 2.0;

/// A waveform widget's GPU view plus its own navigation window.
pub(crate) struct WaveformSlot {
    pub(crate) view: WaveformView,
    pub(crate) nav: View,
}

/// A `WaveformSlot` (GPU view + a fresh full-range nav) for ready data.
pub(crate) fn waveform_slot(data: WaveformData, gpu: &Gpu) -> WaveformSlot {
    let nav = View::full(data.total_samples());
    let view = WaveformView::new(&gpu.device, gpu.config.format, data);
    WaveformSlot { view, nav }
}

/// A placed `plot` widget and the data its (static) draw needs, copied out of
/// the host tree so the mesh is built after the tree borrow is released.
struct PlotItem {
    rect: Rect,
    samples: Arc<[f32]>,
    min: f32,
    max: f32,
    label: Option<String>,
}

/// A placed `canvas` widget, copied out of the host tree: its viewport body, the
/// shader source (for an in-place recompile when it changed) and the param
/// vector, with the bus-mapped slots already resolved from shared memory.
struct CanvasFrame {
    id: i32,
    body: Rect,
    shader: String,
    params: [f32; canvas::PARAM_COUNT],
}

/// The live inputs the frame needs beyond the tree and the GPU resources. The
/// native front fills them from its state; the browser front (G12) passes the
/// defaults (no bus, no node tree, no held button).
pub(crate) struct FrameInputs<'a> {
    /// The control-bus source for `meter`/`canvas` reads (`None` reads zero).
    pub(crate) bus: Option<&'a dyn BusSource>,
    /// The server node trees the `nodetree` view draws, by group.
    pub(crate) node_trees: &'a HashMap<i32, NodeTree>,
    /// The id of a momentary button currently held down (drawn pressed).
    pub(crate) active_button: Option<i32>,
    /// Whether an audio server is attached (the `nodetree` placeholder text).
    pub(crate) server_attached: bool,
}

impl Default for FrameInputs<'_> {
    fn default() -> Self {
        // A 'static empty map for the no-transport (browser, G12) case.
        static EMPTY: std::sync::OnceLock<HashMap<i32, NodeTree>> = std::sync::OnceLock::new();
        Self {
            bus: None,
            node_trees: EMPTY.get_or_init(HashMap::new),
            active_button: None,
            server_attached: false,
        }
    }
}

/// The current value of control bus `bus` from `source` (`0.0` without a source
/// or for a negative/out-of-range bus) — the same rule the native front used.
fn read_bus(source: Option<&dyn BusSource>, bus: i32) -> f32 {
    if bus < 0 {
        return 0.0;
    }
    source.map_or(0.0, |s| s.control(bus as usize))
}

/// Renders `tree` into `gpu`'s surface, using the window's `painter`, `waveforms`
/// and `canvases` resources and (read-only) `scopes` histories, plus `inputs` for
/// the live values. One immutable mesh-building pass over the placed widgets,
/// then the GPU uploads and the single render pass.
pub(crate) fn render(
    gpu: &mut Gpu,
    painter: &mut Painter,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    scopes: &HashMap<i32, VecDeque<f32>>,
    tree: &Widget,
    inputs: &FrameInputs,
) {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    let placed = layout::layout(area, tree);
    let mut mesh = Mesh::new();
    let mut waveform_rects: Vec<(i32, Rect)> = Vec::new();
    // Meter/scope rects, copied out so their shared-memory values and the scope
    // history can be read after the host-tree borrow is released.
    let mut meter_rects: Vec<(Rect, i32, f32, f32, Option<String>)> = Vec::new();
    // Scope rects carry no bus: the value is sampled on the frame tick
    // (`advance_scopes`); the render only draws the stored history.
    let mut scope_rects: Vec<(i32, Rect, f32, f32, Option<String>)> = Vec::new();
    // Plot items (with a cheap Arc clone of the samples) and node-tree rects,
    // likewise copied out so the host-tree borrow can be released before the
    // node-tree models and the GPU resources are read.
    let mut plot_rects: Vec<PlotItem> = Vec::new();
    let mut nodetree_rects: Vec<(Rect, i32, bool, Option<String>)> = Vec::new();
    let mut canvas_frames: Vec<CanvasFrame> = Vec::new();
    let active_button = inputs.active_button;
    for p in &placed {
        match &p.widget.kind {
            WidgetKind::Panel { .. } => mesh.rect(p.rect, PANEL_COLOR),
            WidgetKind::Label { text } => {
                font_left(&mut mesh, text, p.rect);
            }
            WidgetKind::Waveform { .. } => {
                if let Some(id) = p.widget.id {
                    waveform_rects.push((id, p.rect));
                }
            }
            WidgetKind::Meter {
                bus,
                min,
                max,
                label,
            } => meter_rects.push((p.rect, *bus, *min, *max, label.clone())),
            WidgetKind::Scope {
                min, max, label, ..
            } => {
                if let Some(id) = p.widget.id {
                    scope_rects.push((id, p.rect, *min, *max, label.clone()));
                }
            }
            WidgetKind::Plot {
                samples,
                min,
                max,
                label,
                ..
            } => plot_rects.push(PlotItem {
                rect: p.rect,
                samples: Arc::clone(samples),
                min: *min,
                max: *max,
                label: label.clone(),
            }),
            WidgetKind::NodeTree {
                group,
                controls,
                label,
            } => nodetree_rects.push((p.rect, *group, *controls, label.clone())),
            WidgetKind::Canvas {
                shader,
                params,
                buses,
                label,
            } => {
                if let Some(id) = p.widget.id {
                    if let Some(text) = label {
                        super::font::text(
                            &mut mesh,
                            text,
                            p.rect.x + 4.0,
                            p.rect.y + 4.0,
                            LABEL_SCALE,
                            LABEL_COLOR,
                        );
                    }
                    // Resolve the param vector: a `-1` slot keeps its script-set
                    // value; a bus slot is read from shared memory this frame
                    // (zero messages, like a meter).
                    let mut resolved = *params;
                    for (slot, &bus) in resolved.iter_mut().zip(buses.iter()) {
                        if bus >= 0 {
                            *slot = read_bus(inputs.bus, bus);
                        }
                    }
                    canvas_frames.push(CanvasFrame {
                        id,
                        body: controls::body_rect(p.rect, label.is_some()),
                        shader: shader.clone(),
                        params: resolved,
                    });
                }
            }
            WidgetKind::Window { .. } | WidgetKind::Unknown(_) => {}
            kind => controls::draw(&mut mesh, kind, p.rect, p.widget.id == active_button),
        }
    }

    // Meters and scopes read their control bus straight from shared memory each
    // frame (zero messages); the scope keeps a per-widget rolling history in this
    // window's state.
    for (rect, bus, min, max, label) in &meter_rects {
        let value = read_bus(inputs.bus, *bus);
        let frac = meters::fraction(value, *min, *max);
        meters::draw_meter(&mut mesh, *rect, value, frac, label.as_deref());
    }
    // The history is advanced on the frame tick (`advance_scopes`), not here, so a
    // repaint only ever *draws* the current samples — never adds one.
    for (id, rect, min, max, label) in &scope_rects {
        let samples: Vec<f32> = scopes
            .get(id)
            .map(|h| h.iter().copied().collect())
            .unwrap_or_default();
        meters::draw_scope(&mut mesh, *rect, &samples, *min, *max, label.as_deref());
    }

    // Static plots draw from their (already mapped) samples; node trees draw from
    // the model last read off the client leg. Both are pure mesh work with the
    // host-tree borrow already released.
    for item in &plot_rects {
        plot::draw(
            &mut mesh,
            item.rect,
            &item.samples,
            item.min,
            item.max,
            item.label.as_deref(),
        );
    }
    for (rect, group, controls, label) in &nodetree_rects {
        nodetree::draw(
            &mut mesh,
            *rect,
            inputs.node_trees.get(group),
            *controls,
            label.as_deref(),
            inputs.server_attached,
        );
    }

    painter.upload(&gpu.device, &gpu.queue, &mesh, fb_w, fb_h);
    for (id, rect) in &waveform_rects {
        if let Some(slot) = waveforms.get_mut(id) {
            slot.view
                .upload(&gpu.device, &gpu.queue, &slot.nav, rect.w.max(1.0) as u32);
        }
    }
    // Recompile any canvas whose shader changed, then push its per-frame uniforms
    // (viewport size, elapsed time, resolved params).
    for frame in &canvas_frames {
        if let Some(view) = canvases.get_mut(&frame.id) {
            view.set_shader(&gpu.device, &frame.shader);
            let time = view.elapsed();
            let res = [frame.body.w.max(1.0), frame.body.h.max(1.0)];
            view.upload(&gpu.queue, res, time, frame.params);
        }
    }

    let frame = match gpu.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        _ => {
            gpu.surface.configure(&gpu.device, &gpu.config);
            return;
        }
    };
    let target = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gui frame"),
        });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gui pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        painter.draw(&mut pass);
        for (id, rect) in &waveform_rects {
            if rect.w >= 1.0
                && rect.h >= 1.0
                && let Some(slot) = waveforms.get(id)
            {
                let (x, y, w, h) = clamp_viewport(*rect, fb_w, fb_h);
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                slot.view.draw(&mut pass);
            }
        }
        for frame in &canvas_frames {
            if frame.body.w >= 1.0
                && frame.body.h >= 1.0
                && let Some(view) = canvases.get(&frame.id)
            {
                let (x, y, w, h) = clamp_viewport(frame.body, fb_w, fb_h);
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                view.draw(&mut pass);
            }
        }
    }
    gpu.queue.submit(std::iter::once(encoder.finish()));
    frame.present();
}

/// Draws `text` vertically centered at the left of `rect` (a label).
fn font_left(mesh: &mut Mesh, text: &str, rect: Rect) {
    let y = rect.y + (rect.h - super::font::height(LABEL_SCALE)) * 0.5;
    super::font::text(
        mesh,
        text,
        rect.x + 4.0,
        y.max(rect.y),
        LABEL_SCALE,
        LABEL_COLOR,
    );
}

/// Clamps a widget rect to the framebuffer for `set_viewport` (which rejects a
/// viewport that leaves the attachment).
fn clamp_viewport(r: Rect, fb_w: u32, fb_h: u32) -> (f32, f32, f32, f32) {
    let x = r.x.clamp(0.0, fb_w as f32);
    let y = r.y.clamp(0.0, fb_h as f32);
    let w = r.w.min(fb_w as f32 - x).max(0.0);
    let h = r.h.min(fb_h as f32 - y).max(0.0);
    (x, y, w, h)
}
