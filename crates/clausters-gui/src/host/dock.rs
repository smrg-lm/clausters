//! **The dock**: several windows drawn into one surface, each in a
//! rectangle of it -- one window of the platform holding every window the
//! host opens, natively and in a page alike.
//!
//! A window in a dock is a **pane**. It keeps everything a window of its
//! own has -- its def, its instance, its history, its gesture machine, its
//! replies -- and loses only the surface: its frames are drawn into a texture
//! of its own on the dock's device ([`Gpu::pane`](crate::gpu::Gpu)),
//! through the very passes a window's frame takes, and the dock copies
//! each texture into its place in the surface it presents ([`Composer`]).
//! So a pane's picture is the picture the same window would show on its own,
//! at the same size, by construction.
//!
//! Two halves, both shared by the fronts:
//!
//! - [`Dock`] is the arrangement and the hand: where each pane is, which
//!   pane the pointer is over, which one a press holds until its release and
//!   which one the keyboard goes to. Pure logic, with no device and no window.
//! - [`Composer`] is the copy of every pane into the surface, and a picture
//!   of the whole read back (`Composer::capture`, for the tests).
//!
//! The arrangement here is the simplest one that holds several panes: equal
//! columns, side by side, a divider between them. Splits, tabs and moving a
//! pane are the next layer over the same two halves.

use clausters_gui_draw::theme::Theme;

use crate::gpu::Gpu;
use crate::host::frame::Copier;
#[cfg(test)]
use crate::host::frame::{Readback, read_back, read_target};
use crate::view::Target;

/// **The color between two panes**: the theme's `separator` role, what the
/// dock clears its surface to before it copies the panes in.
pub(crate) fn divider_color(theme: &Theme) -> wgpu::Color {
    let [r, g, b, a] = theme.separator;
    wgpu::Color {
        r: f64::from(r),
        g: f64::from(g),
        b: f64::from(b),
        a: f64::from(a),
    }
}

/// A pane's rectangle in the dock's surface, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PaneRect {
    /// Whether the point `(x, y)` of the surface is inside.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= f64::from(self.x)
            && y >= f64::from(self.y)
            && x < f64::from(self.x + self.w)
            && y < f64::from(self.y + self.h)
    }
}

/// Where a pointer's motion goes: the pane it reaches, with the point in the
/// pane's own pixels, and the pane it left on the way, if it left one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub to: Option<(i32, (f64, f64))>,
    pub left: Option<i32>,
}

/// **The arrangement of the panes, and where the hand is among them.**
#[derive(Debug, Default)]
pub struct Dock {
    /// The panes, in the order they are placed (left to right).
    panes: Vec<i32>,
    /// Each pane's rectangle, in the order of `panes`.
    rects: Vec<PaneRect>,
    /// The surface's size, in device pixels.
    size: (u32, u32),
    /// The width of the divider between two panes, in device pixels.
    divider: u32,
    /// The pane the pointer is over, with no button held.
    pointer: Option<i32>,
    /// The pane a press landed in: every motion and the release go to it,
    /// wherever the pointer is, until the button comes up -- a drag owns
    /// the pointer, in a pane as in a window.
    held: Option<i32>,
    /// The pane the keyboard goes to: the one last pressed in.
    focus: Option<i32>,
}

impl Dock {
    /// An empty dock of `size`, with `divider` pixels between panes.
    pub fn new(size: (u32, u32), divider: u32) -> Self {
        Self {
            size,
            divider,
            ..Self::default()
        }
    }

    /// The panes, in the order they are placed.
    pub fn panes(&self) -> &[i32] {
        &self.panes
    }

    /// Whether `def_id` is a pane here.
    pub fn holds(&self, def_id: i32) -> bool {
        self.panes.contains(&def_id)
    }

    /// The surface's size.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The divider's width.
    pub fn divider(&self) -> u32 {
        self.divider
    }

    /// Adds `def_id` as the last pane, and gives it the keyboard. Answers
    /// whether it was not one already.
    pub fn add(&mut self, def_id: i32) -> bool {
        if self.holds(def_id) {
            return false;
        }
        self.panes.push(def_id);
        self.focus = Some(def_id);
        self.arrange();
        true
    }

    /// Takes `def_id` out; the panes left share its room. Answers whether it
    /// was one.
    pub fn remove(&mut self, def_id: i32) -> bool {
        let Some(at) = self.panes.iter().position(|&p| p == def_id) else {
            return false;
        };
        self.panes.remove(at);
        for slot in [&mut self.pointer, &mut self.held, &mut self.focus] {
            if *slot == Some(def_id) {
                *slot = None;
            }
        }
        if self.focus.is_none() {
            self.focus = self.panes.last().copied();
        }
        self.arrange();
        true
    }

    /// The surface's size changed. Answers whether it did.
    pub fn set_size(&mut self, size: (u32, u32)) -> bool {
        if self.size == size {
            return false;
        }
        self.size = size;
        self.arrange();
        true
    }

    /// The divider's width changed (a display of another density). Answers
    /// whether it did.
    pub fn set_divider(&mut self, divider: u32) -> bool {
        if self.divider == divider {
            return false;
        }
        self.divider = divider;
        self.arrange();
        true
    }

    /// Pane `def_id`'s rectangle.
    pub fn rect(&self, def_id: i32) -> Option<PaneRect> {
        let at = self.panes.iter().position(|&p| p == def_id)?;
        self.rects.get(at).copied()
    }

    /// Every pane with its rectangle, in the order they are placed.
    pub fn placed(&self) -> impl Iterator<Item = (i32, PaneRect)> + '_ {
        self.panes.iter().copied().zip(self.rects.iter().copied())
    }

    /// The pane under the point `(x, y)` of the surface, and the point in its
    /// own pixels. A divider is in no pane.
    pub fn at(&self, x: f64, y: f64) -> Option<(i32, (f64, f64))> {
        self.placed()
            .find(|(_, r)| r.contains(x, y))
            .map(|(id, r)| (id, (x - f64::from(r.x), y - f64::from(r.y))))
    }

    /// The point `(x, y)` of the surface in pane `def_id`'s own pixels,
    /// wherever it is -- a drag that left the pane still reads it there.
    pub fn local(&self, def_id: i32, x: f64, y: f64) -> Option<(f64, f64)> {
        self.rect(def_id)
            .map(|r| (x - f64::from(r.x), y - f64::from(r.y)))
    }

    /// The pane the keyboard goes to.
    pub fn focus(&self) -> Option<i32> {
        self.focus
    }

    /// The pane the pointer is over, or the one a press holds.
    pub fn pointer(&self) -> Option<i32> {
        self.held.or(self.pointer)
    }

    /// Whether a press holds a pane.
    pub fn held(&self) -> Option<i32> {
        self.held
    }

    /// **The pointer moved to `(x, y)`**: the pane a press holds, if one
    /// does, else the pane under it -- and the pane it left, which hears it
    /// left, as a window hears the pointer leave it.
    pub fn moved(&mut self, x: f64, y: f64) -> Motion {
        if let Some(held) = self.held {
            return Motion {
                to: self.local(held, x, y).map(|p| (held, p)),
                left: None,
            };
        }
        let to = self.at(x, y);
        let now = to.map(|(id, _)| id);
        let left = self.pointer.filter(|&was| Some(was) != now);
        self.pointer = now;
        Motion { to, left }
    }

    /// **A button went down**: the pane under the pointer takes it, and the
    /// keyboard with it. Answers that pane.
    pub fn pressed(&mut self) -> Option<i32> {
        let pane = self.held.or(self.pointer)?;
        self.held = Some(pane);
        self.focus = Some(pane);
        Some(pane)
    }

    /// **The button came up**: answers the pane the press held, which hears
    /// the release wherever the pointer is.
    pub fn released(&mut self) -> Option<i32> {
        self.held.take().or(self.pointer)
    }

    /// **The pointer left the surface**: answers the pane it was over, unless
    /// a press holds one -- motion keeps reaching that until the release.
    pub fn left(&mut self) -> Option<i32> {
        if self.held.is_some() {
            return None;
        }
        self.pointer.take()
    }

    /// Equal columns, a divider between each two, the last taking what the
    /// division left over.
    fn arrange(&mut self) {
        let n = self.panes.len() as u32;
        self.rects.clear();
        if n == 0 {
            return;
        }
        let (w, h) = self.size;
        let dividers = self.divider * (n - 1);
        let room = w.saturating_sub(dividers);
        let each = room / n;
        let mut x = 0;
        for i in 0..n {
            let width = if i + 1 == n {
                w.saturating_sub(x).max(1)
            } else {
                each.max(1)
            };
            self.rects.push(PaneRect {
                x,
                y: 0,
                w: width,
                h: h.max(1),
            });
            x += width + self.divider;
        }
    }
}

/// **The copy of every pane into the dock's surface**: each pane's
/// texture into its rectangle, the dividers left in the clear color.
pub(crate) struct Composer {
    copier: Copier,
    /// One shift buffer a pane, reused frame to frame.
    shifts: Vec<wgpu::Buffer>,
}

impl Composer {
    /// A composer drawing into a single-sampled target of `format`.
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            copier: Copier::new(device, Target::new(format)),
            shifts: Vec::new(),
        }
    }

    /// **Encodes the copy of `panes` into `target`**, the dock's
    /// surface or a texture of its size, cleared to `clear` first: each
    /// pane's texture into its rectangle, one texel to one pixel.
    /// `dock` is the dock's GPU, whose configuration is the
    /// surface's size.
    pub(crate) fn compose(
        &mut self,
        dock: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clear: wgpu::Color,
        panes: &[(PaneRect, &wgpu::TextureView)],
    ) {
        let (device, queue) = (&dock.device, &dock.queue);
        let size = (dock.config.width, dock.config.height);
        while self.shifts.len() < panes.len() {
            self.shifts
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pane shift"),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        let groups: Vec<_> = panes
            .iter()
            .zip(&self.shifts)
            .map(|((rect, view), shift)| {
                let offset = [-(rect.x as f32), -(rect.y as f32), 0.0, 0.0];
                queue.write_buffer(shift, 0, bytemuck::cast_slice(&offset));
                self.copier.bind(device, view, shift)
            })
            .collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("dock pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.copier.single);
        let (sw, sh) = (size.0.max(1), size.1.max(1));
        for ((rect, _), group) in panes.iter().zip(&groups) {
            // a pane is never drawn past the surface, whatever it was sized
            // to before the surface shrank
            let x = rect.x.min(sw);
            let y = rect.y.min(sh);
            let w = rect.w.min(sw - x);
            let h = rect.h.min(sh - y);
            if w == 0 || h == 0 {
                continue;
            }
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        // Left over all of the target: a blit WebGL presents through is
        // clipped by the scissor still set.
        pass.set_viewport(0.0, 0.0, sw as f32, sh as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, sw, sh);
    }

    /// **The dock's picture, read back**: the panes composed into a
    /// texture of `size` instead of the surface, as a window's capture is its
    /// frame drawn into a texture. What the tests compare against each
    /// pane's own picture; a script captures a pane, which is a window.
    #[cfg(test)]
    pub(crate) fn capture(
        &mut self,
        dock: &Gpu,
        clear: wgpu::Color,
        panes: &[(PaneRect, &wgpu::TextureView)],
    ) -> Result<Readback, String> {
        let size = (dock.config.width, dock.config.height);
        let texture = read_target(&dock.device, dock.config.format, size)?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = dock
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("dock capture"),
            });
        self.compose(dock, &mut encoder, &view, clear, panes);
        Ok(read_back(&dock.device, &dock.queue, encoder, &texture))
    }
}

#[cfg(test)]
mod tests;
