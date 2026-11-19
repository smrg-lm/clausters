//! **The dock, natively**: one window of the platform whose surface
//! shows every window the host opens as a pane of it
//! ([`crate::host::dock`]), when the host was asked for one
//! (`Host::dock`).
//!
//! A pane is a window in every way but its surface: it has its `WindowState`,
//! its gesture machine and its replies, and [`App::def_event`] is what it
//! hears. What this module adds is the one window and its routing -- the
//! platform's events arrive for the dock's window, and each goes to the
//! pane it reaches, in that pane's own pixels, or to the dock's own chrome
//! (a tab, a divider) -- the frame that copies every shown pane into the
//! surface with the chrome over it, and the dock's door on the wire
//! (`/gui_dock`).
//!
//! A window taken out of the dock is a **floating panel**: a window of the
//! platform of its own, drawn as the dock draws a panel -- its pane under
//! its bar ([`Dock::floating`]) -- so it is a shell like the dock's, with
//! one pane and no pages ([`Which::Float`]).

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};
use tracing::{info, warn};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::host::dock::call::{self, Call};
use crate::host::dock::{Composer, Dock, Handed, Which, divider_color};
use crate::host::{ClientId, PointerKind, PointerMods, png};

use super::Loop;
use super::app::App;

/// The size a dock's window opens at, in logical pixels.
const OPENS_AT: (u32, u32) = (1280, 800);

/// The shell `which` names, borrowing only the field it lives in.
macro_rules! shell_mut {
    ($app:expr, $which:expr) => {
        match $which {
            Which::Main => $app.dock.as_mut(),
            Which::Float(id) => $app.floats.get_mut(&id),
        }
    };
}

/// The dock's window, its surface and its panes.
pub(in crate::host::gui) struct Shell {
    gpu: Gpu,
    layout: Dock,
    composer: Composer,
    /// The window's display scale, which every pane is laid out at.
    scale: f32,
    /// The panes behind another's tab, told they are not seen.
    hidden: HashSet<i32>,
    /// The resize cursor the window shows ([`Dock::resize_cursor`]).
    cursor: Option<bool>,
    /// The modifiers held, as the platform last said them: a pane made
    /// while a key is held (Ctrl+N's new file) was not there to be told.
    mods: Option<Modifiers>,
}

impl App {
    /// The shell whose window is `id`: the dock's, or a floating panel's.
    pub(super) fn shell_of(&self, id: WindowId) -> Option<Which> {
        let is = |s: &Shell| s.gpu.window().is_some_and(|w| w.id() == id);
        if self.dock.as_ref().is_some_and(is) {
            return Some(Which::Main);
        }
        self.floats
            .iter()
            .find(|(_, s)| is(s))
            .map(|(&def, _)| Which::Float(def))
    }

    /// Whether window `def_id` is a pane of the dock.
    pub(super) fn is_pane(&self, def_id: i32) -> bool {
        self.dock.as_ref().is_some_and(|s| s.layout.holds(def_id))
    }

    /// **The GPU of window `id` as a pane of the dock**, the dock's
    /// window opened first if it is not open yet. The panes already there
    /// make room for it.
    pub(super) fn open_pane(&mut self, lp: Loop, id: i32) -> Option<Gpu> {
        if self.dock.is_none() {
            let (title, size) = self
                .host
                .dock_opens
                .clone()
                .unwrap_or_else(|| ("clausters".into(), OPENS_AT));
            self.dock = Some(self.open_shell(lp, &title, size, false)?);
        }
        let look = self.host.pane_look(id);
        let page = self.host.window_dock_page(id);
        let edge = self.host.window_dock_edge(id);
        let shell = self.dock.as_mut()?;
        match edge {
            // the application's bar or footer, along an edge
            Some((edge, h)) => {
                shell.layout.add_edge(id, edge, px(h, shell.scale));
            }
            None => {
                shell.layout.add_on(id, page.as_deref());
            }
        }
        shell.layout.describe(id, look);
        self.host.set_ui_scale(id, shell.scale);
        let rect = shell.layout.rect(id)?;
        let gpu = Gpu::pane(&shell.gpu, rect.w, rect.h);
        shell.gpu.request_redraw();
        self.settle_dock();
        Some(gpu)
    }

    /// **The GPU of window `id` as a floating panel**: a window of the
    /// platform of its own, of the size the window declares with its
    /// panel's bar above it, which draws the pane under the bar.
    pub(super) fn open_float(&mut self, lp: Loop, id: i32, (w, h): (u32, u32)) -> Option<Gpu> {
        let title = self.host.window_title(id).unwrap_or_default();
        let bar = Dock::floating((1, 1), self.host.metrics.resolved(1.0)).bar_height();
        let mut shell = self.open_shell(lp, &title, (w, h + bar), true)?;
        self.host.set_ui_scale(id, shell.scale);
        // the window's own size at its scale, under the bar at that scale
        if let (Some(window), Some((pw, ph))) = (shell.gpu.window(), self.host.window_size_px(id)) {
            let bar = shell.layout.bar_height();
            let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(pw, ph + bar));
        }
        shell.layout.add(id);
        shell.layout.describe(id, self.host.pane_look(id));
        let rect = shell.layout.rect(id)?;
        let gpu = Gpu::pane(&shell.gpu, rect.w, rect.h);
        shell.gpu.request_redraw();
        self.floats.insert(id, shell);
        self.settle_dock();
        Some(gpu)
    }

    /// **The dock's clock**: a tab rested on long enough shows its tip.
    /// Answers whether one is still waiting, which keeps the loop coming
    /// back.
    pub(super) fn advance_dock_timers(&mut self) -> bool {
        let now = self.started.elapsed().as_secs_f64() * 1000.0;
        let mut waiting = false;
        for shell in self.dock.iter_mut().chain(self.floats.values_mut()) {
            if shell.layout.waiting() {
                // a tip that came due is drawn
                if shell.layout.elapsed(now) {
                    shell.gpu.request_redraw();
                }
                waiting |= shell.layout.waiting();
            }
        }
        waiting
    }

    /// **What the dock shows of pane `id` changed** (its title, whether it
    /// may be closed, its changes not kept, its tab's tip -- or anything a
    /// redefinition moved): its panel says it, and a floating panel's
    /// window its title too.
    pub(super) fn retitle_pane(&mut self, id: i32) {
        let look = self.host.pane_look(id);
        if let Some(shell) = self.floats.get_mut(&id) {
            if let (Some(window), Some(title)) = (shell.gpu.window(), &look.title) {
                window.set_title(title);
            }
            shell.layout.describe(id, look);
        } else if let Some(shell) = self.dock.as_mut()
            && shell.layout.holds(id)
        {
            shell.layout.describe(id, look);
        }
        self.settle_dock();
    }

    /// **Window `id` is no longer a pane**: the others share its room, and
    /// the dock's window closes with its last pane; a floating panel's
    /// window closes with it.
    pub(super) fn close_pane(&mut self, id: i32) {
        if self.floats.remove(&id).is_some() {
            return;
        }
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        if !shell.layout.remove(id) {
            return;
        }
        shell.hidden.remove(&id);
        self.host.set_popup_area(id, None);
        self.host.set_header_band(id, None);
        if shell.layout.panes().is_empty() {
            self.dock = None;
            return;
        }
        self.settle_dock();
    }

    /// A shell's window and its GPU, `size` logical pixels: on the screen,
    /// or offscreen for a headless host. The dock's, or a floating panel's
    /// (`floating`).
    fn open_shell(
        &mut self,
        lp: Loop,
        title: &str,
        size: (u32, u32),
        floating: bool,
    ) -> Option<Shell> {
        let (gpu, scale) = match lp {
            Loop::Shown(event_loop) => {
                let attrs = Window::default_attributes()
                    .with_title(title)
                    .with_inner_size(LogicalSize::new(f64::from(size.0), f64::from(size.1)));
                let window = match event_loop.create_window(attrs) {
                    Ok(w) => Arc::new(w),
                    Err(e) => {
                        warn!("dock: cannot create its window: {e}");
                        return None;
                    }
                };
                let scale = window.scale_factor() as f32;
                match pollster::block_on(Gpu::new(window, self.host.msaa)) {
                    Ok(gpu) => (gpu, scale),
                    Err(e) => {
                        warn!("dock: cannot start the GPU: {e}");
                        return None;
                    }
                }
            }
            Loop::Headless(_) => {
                let scale = self.host.headless_scale.max(0.5);
                let (w, h) = (
                    (size.0 as f32 * scale).round() as u32,
                    (size.1 as f32 * scale).round() as u32,
                );
                match pollster::block_on(Gpu::offscreen(w, h, self.host.msaa)) {
                    Ok(gpu) => (gpu, scale),
                    Err(e) => {
                        warn!("dock: cannot start the GPU: {e}");
                        return None;
                    }
                }
            }
        };
        let size = (gpu.config.width.max(1), gpu.config.height.max(1));
        let metrics = self.host.metrics.resolved(scale);
        info!(
            "dock: opened {} at {}x{} on {}",
            if floating {
                "a floating panel"
            } else {
                "the dock"
            },
            size.0,
            size.1,
            gpu.adapter
        );
        Some(Shell {
            composer: Composer::new(&gpu.device, gpu.config.format),
            layout: if floating {
                Dock::floating(size, metrics)
            } else {
                Dock::new(size, metrics)
            },
            gpu,
            scale,
            hidden: HashSet::new(),
            cursor: None,
            mods: None,
        })
    }

    /// **Every shell brought up to what changed in it**: every pane at the
    /// size of its place (a pane whose rectangle changed is resized and
    /// draws again whole), a pane behind another's tab told it is not seen
    /// and one shown again told it is, and a shell drawn if anything of it
    /// moved.
    ///
    /// Answers what a hand asked of the windows themselves -- the panes
    /// taken out of the dock, those closed, those put back -- for the caller
    /// to carry out ([`App::hand_over`]).
    fn settle_dock(&mut self) -> Handed {
        let App {
            dock,
            floats,
            windows,
            host,
            ..
        } = self;
        let mut handed = Handed::default();
        for shell in dock.iter_mut().chain(floats.values_mut()) {
            let dirty = shell.layout.take_dirty();
            if dirty.layout {
                for id in shell.layout.panes() {
                    // its popups go anywhere over the shell
                    host.set_popup_area(id, shell.layout.popup_area(id));
                    let shown = shell.layout.shown(id);
                    let rect = shell.layout.rect(id);
                    let Some(ws) = windows.get_mut(&id) else {
                        continue;
                    };
                    if let Some(rect) = rect
                        && (ws.gpu.config.width, ws.gpu.config.height) != (rect.w, rect.h)
                    {
                        ws.gpu.resize(rect.w, rect.h);
                        ws.repaint();
                    }
                    // a hidden tab is a covered window: no tick, no frame
                    if shown == shell.hidden.contains(&id) {
                        ws.occluded = !shown;
                        if shown {
                            shell.hidden.remove(&id);
                            ws.repaint();
                        } else {
                            shell.hidden.insert(id);
                        }
                    }
                }
            }
            if dirty.layout || dirty.chrome {
                // a pane filling its panel's header is told where the bar
                // leaves it room, which its tabs and its title move
                for id in shell.layout.panes() {
                    if host.set_header_band(id, shell.layout.band(id))
                        && let Some(ws) = windows.get(&id)
                    {
                        ws.repaint();
                    }
                }
                shell.gpu.request_redraw();
            }
            // the bar shows the menu of the application on the page shown
            for id in host.show_menus(&shell.layout) {
                if let Some(ws) = windows.get(&id) {
                    ws.repaint();
                }
            }
            // a divider under the pointer, or in the hand, says it is dragged
            let cursor = shell.layout.resize_cursor();
            if cursor != shell.cursor {
                shell.cursor = cursor;
                if let Some(window) = shell.gpu.window() {
                    window.set_cursor(resize_icon(cursor));
                }
            }
            handed.take(dirty);
        }
        handed
    }

    /// **What a hand asked of the windows of a shell**: each pane taken out
    /// gets a window of its own, each one put back is a pane again, and each
    /// one closed closes as its own close mark would close it (its owner
    /// asked first where it asks).
    fn hand_over(&mut self, lp: Loop, handed: Handed) {
        for id in handed.dismiss {
            if self.host.close_popup(id)
                && let Some(ws) = self.windows.get(&id)
            {
                ws.repaint();
            }
        }
        for (ids, into) in [(handed.out, false), (handed.back, true)] {
            for id in ids {
                if let Err(e) = self.move_window(lp, id, into) {
                    warn!("dock: {e}");
                }
            }
        }
        for id in handed.close {
            self.user_close(id, lp);
        }
    }

    /// **What the platform said of the dock's window**, handed to the
    /// pane it is about: the motion and the buttons to the pane under the
    /// pointer (or the one a press holds), the keys to the pane last pressed
    /// in, what is the whole window's -- its size, its scale, the modifiers,
    /// being seen -- to the dock or to every pane.
    pub(super) fn dock_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        which: Which,
        event: WindowEvent,
    ) {
        let Some(shell) = shell_mut!(self, which) else {
            return;
        };
        let panes = shell.layout.panes().to_vec();
        if let WindowEvent::ModifiersChanged(mods) = &event {
            shell.mods = Some(*mods);
        }
        let mods = shell.mods;
        match event {
            WindowEvent::RedrawRequested => self.compose_dock(which),
            // the dock's window is the application's: its floating panels
            // close with it
            WindowEvent::CloseRequested => {
                let floats: Vec<i32> = match which {
                    Which::Main => self.floats.keys().copied().collect(),
                    Which::Float(_) => Vec::new(),
                };
                for id in panes.into_iter().chain(floats) {
                    self.user_close(id, Loop::Shown(event_loop));
                }
            }
            WindowEvent::Resized(size) => {
                shell.gpu.resize(size.width, size.height);
                shell
                    .layout
                    .set_size((size.width.max(1), size.height.max(1)));
                shell.gpu.request_redraw();
                self.settle_dock();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                shell.scale = scale_factor as f32;
                shell
                    .layout
                    .set_metrics(self.host.metrics.resolved(shell.scale));
                for &id in &panes {
                    if let Some((edge, h)) = self.host.window_dock_edge(id) {
                        shell.layout.add_edge(id, edge, px(h, shell.scale));
                    }
                    self.host.set_ui_scale(id, scale_factor as f32);
                    if let Some(ws) = self.windows.get(&id) {
                        ws.repaint();
                    }
                }
                self.settle_dock();
            }
            WindowEvent::ModifiersChanged(_)
            | WindowEvent::Occluded(_)
            | WindowEvent::Focused(_) => {
                for id in panes {
                    self.def_event(event_loop, id, event.clone());
                }
            }
            WindowEvent::CursorMoved {
                device_id,
                position,
            } => {
                let motion = shell.layout.moved(position.x, position.y);
                if let Some(left) = motion.left {
                    self.def_event(event_loop, left, WindowEvent::CursorLeft { device_id });
                }
                if let Some((id, (x, y))) = motion.to {
                    let position = PhysicalPosition::new(x, y);
                    self.def_event(
                        event_loop,
                        id,
                        WindowEvent::CursorMoved {
                            device_id,
                            position,
                        },
                    );
                }
                self.settle_dock();
            }
            WindowEvent::CursorLeft { device_id } => {
                if let Some(id) = shell.layout.left() {
                    self.def_event(event_loop, id, WindowEvent::CursorLeft { device_id });
                }
                self.settle_dock();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // the primary button's press holds its pane until it comes up
                let pane = match (state, button) {
                    (ElementState::Pressed, MouseButton::Left) => shell.layout.pressed(),
                    (ElementState::Released, MouseButton::Left) => shell.layout.released(),
                    // a panel's bar or a page's tab opens its menu
                    (ElementState::Pressed, MouseButton::Right)
                        if shell.layout.context_pressed() =>
                    {
                        None
                    }
                    // the middle button on a tab closes its window
                    (ElementState::Pressed, MouseButton::Middle)
                        if shell.layout.middle_pressed() =>
                    {
                        None
                    }
                    _ => shell.layout.pointer(),
                };
                if let Some(id) = pane {
                    self.input_event(event_loop, id, mods, event);
                }
                let handed = self.settle_dock();
                self.hand_over(Loop::Shown(event_loop), handed);
            }
            WindowEvent::MouseWheel { .. } => {
                if let Some(id) = shell.layout.pointer() {
                    self.input_event(event_loop, id, mods, event);
                }
            }
            // Escape takes down the dock's open menu before any pane hears it
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } if shell.layout.menu().is_some() => {
                shell.layout.escape();
                self.settle_dock();
            }
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) => {
                if let Some(id) = shell.layout.focus() {
                    self.input_event(event_loop, id, mods, event);
                }
            }
            _ => {}
        }
    }

    /// **A button or a key handed to pane `id`**, after the modifiers held
    /// now: whichever pane it reaches reads them as they are, though it was
    /// made, or shown, after they were pressed.
    fn input_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        id: i32,
        mods: Option<Modifiers>,
        event: WindowEvent,
    ) {
        if let Some(mods) = mods {
            self.def_event(event_loop, id, WindowEvent::ModifiersChanged(mods));
        }
        self.def_event(event_loop, id, event);
    }

    /// **A shell's frame**: each shown pane that asked for a frame draws
    /// it into its texture, then every shown pane is copied into its place
    /// in the surface with the shell's chrome over them.
    fn compose_dock(&mut self, which: Which) {
        let Some(shown) = shell_mut!(self, which)
            .map(|s| s.layout.placed().map(|(id, _)| id).collect::<Vec<_>>())
        else {
            return;
        };
        for id in shown {
            if self
                .windows
                .get(&id)
                .is_some_and(|ws| ws.asked.get().is_some())
            {
                self.render(id);
            }
        }
        let clear = divider_color(&self.host.theme);
        let App {
            dock,
            floats,
            windows,
            host,
            ..
        } = self;
        let shell = match which {
            Which::Main => dock.as_mut(),
            Which::Float(id) => floats.get_mut(&id),
        };
        let Some(shell) = shell else {
            return;
        };
        let Some(surface) = shell.gpu.surface() else {
            return;
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                // no drawable this turn: configured again and asked again,
                // as a window of its own does
                surface.configure(&shell.gpu.device, &shell.gpu.config);
                shell.gpu.request_redraw();
                return;
            }
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let chrome = picture(&mut shell.layout, windows, host);
        let views: Vec<_> = shell
            .layout
            .placed()
            .filter_map(|(id, rect)| {
                windows
                    .get(&id)
                    .and_then(|ws| ws.gpu.pane_view())
                    .map(|view| (rect, view))
            })
            .collect();
        let mut encoder =
            shell
                .gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("dock frame"),
                });
        shell
            .composer
            .compose(&shell.gpu, &mut encoder, &target, clear, &views, &chrome);
        shell.gpu.queue.submit(std::iter::once(encoder.finish()));
        if let Some(window) = shell.gpu.window() {
            window.pre_present_notify();
        }
        frame.present();
    }

    /// **A call to the dock** (`/gui_dock`), carried out and answered to
    /// `from`: a move of the layout, a hand on the surface, the picture.
    pub(super) fn dock_call(&mut self, lp: Loop, from: ClientId, seq: i32, call: Call) {
        let refused = |e: String| ("command".to_string(), e);
        let answer = match (self.dock.is_some(), call) {
            (_, Call::Undock(id)) => self
                .move_window(lp, id, false)
                .map(|()| self.dock_state())
                .map_err(refused),
            (_, Call::Dock(id)) => self
                .move_window(lp, id, true)
                .map(|()| self.dock_state())
                .map_err(refused),
            (
                _,
                Call::Pointer {
                    panel: Some(id), ..
                }
                | Call::Capture {
                    panel: Some(id), ..
                },
            ) if !self.floats.contains_key(&id) => {
                Err(refused(format!("window {id} is not a floating panel")))
            }
            (
                _,
                Call::Pointer {
                    kind,
                    x,
                    y,
                    panel: Some(id),
                },
            ) => {
                let handed = self.dock_pointer(Which::Float(id), kind, x, y);
                self.hand_over(lp, handed);
                Ok(self.dock_state())
            }
            (
                _,
                Call::Capture {
                    path,
                    panel: Some(id),
                },
            ) => self
                .dock_capture(Which::Float(id), &path)
                .map(|()| json!({ "path": path }))
                .map_err(|e| ("runtime".to_string(), e)),
            (false, _) => Err((
                "runtime".to_string(),
                "the dock is not open: it opens with its first pane".to_string(),
            )),
            (true, Call::Pointer { kind, x, y, .. }) => {
                let handed = self.dock_pointer(Which::Main, kind, x, y);
                self.hand_over(lp, handed);
                Ok(self.dock_state())
            }
            (true, Call::Capture { path, .. }) => self
                .dock_capture(Which::Main, &path)
                .map(|()| json!({ "path": path }))
                .map_err(|e| ("runtime".to_string(), e)),
            (true, call) => {
                let answer = match self.dock.as_mut() {
                    Some(shell) => shell.layout.apply(&call),
                    None => Ok(Value::Null),
                };
                self.settle_dock();
                answer.map(|_| self.dock_state())
            }
        };
        self.send(from, call::reply(seq, answer));
    }

    /// The dock as it stands, with the windows taken out of it.
    fn dock_state(&self) -> Value {
        let state = self
            .dock
            .as_ref()
            .map_or_else(call::closed_state, |s| s.layout.state());
        let out: Vec<i32> = self.undocked.iter().copied().collect();
        call::with_out(state, &out)
    }

    /// **A scripted hand on a shell's surface**, at `(x, y)` of it: the
    /// chrome takes what lands on it, and a pane what lands in it, at its own
    /// pixels, through the window's scripted pointer -- the hand the
    /// platform's pointer is.
    ///
    /// Answers what the hand asked of the windows ([`App::hand_over`]).
    fn dock_pointer(&mut self, which: Which, kind: PointerKind, x: f64, y: f64) -> Handed {
        let Some(shell) = shell_mut!(self, which) else {
            return Default::default();
        };
        let layout = &mut shell.layout;
        let motion = layout.moved(x, y);
        let to = match kind {
            PointerKind::Move => motion.to.map(|(id, _)| id),
            PointerKind::Press => layout.pressed(),
            PointerKind::Release => layout.released(),
        };
        let local = to.and_then(|id| layout.local(id, x, y).map(|p| (id, p)));
        if let Some((id, (lx, ly))) = local {
            self.pointer(id, kind, lx, ly, PointerMods::default());
        }
        self.settle_dock()
    }

    /// **A shell's picture at `path`**: every shown pane drawn into its
    /// texture, then composed with the chrome into a texture of the
    /// surface's size, read back and written as a PNG.
    fn dock_capture(&mut self, which: Which, path: &str) -> Result<(), String> {
        let shown: Vec<i32> = shell_mut!(self, which)
            .map(|s| s.layout.placed().map(|(id, _)| id).collect())
            .unwrap_or_default();
        for &id in &shown {
            if let Some(ws) = self.windows.get(&id) {
                ws.batches.want_whole();
            }
            self.render(id);
        }
        let clear = divider_color(&self.host.theme);
        let App {
            dock,
            floats,
            windows,
            host,
            ..
        } = self;
        let shell = match which {
            Which::Main => dock.as_mut(),
            Which::Float(id) => floats.get_mut(&id),
        };
        let shell = shell.ok_or("the dock closed")?;
        let chrome = picture(&mut shell.layout, windows, host);
        let views: Vec<_> = shell
            .layout
            .placed()
            .filter_map(|(id, rect)| {
                windows
                    .get(&id)
                    .and_then(|ws| ws.gpu.pane_view())
                    .map(|view| (rect, view))
            })
            .collect();
        let readback = shell.composer.capture(&shell.gpu, clear, &views, &chrome)?;
        readback.wait(&shell.gpu.device)?;
        let (width, height, rgba) = readback.pixels();
        std::fs::write(path, png::encode_rgba(width, height, &rgba)).map_err(|e| e.to_string())
    }
}

/// **What a shell draws over its panes**: its chrome, then the popups of
/// each shown pane, which float over the whole shell.
fn picture(
    layout: &mut Dock,
    windows: &std::collections::HashMap<i32, super::app::WindowState>,
    host: &crate::host::Host,
) -> crate::host::dock::Overlay {
    let shown: Vec<i32> = layout.placed().map(|(id, _)| id).collect();
    let floats: Vec<crate::host::dock::PaneFloat<'_>> = shown
        .into_iter()
        .filter_map(|id| {
            let ws = windows.get(&id)?;
            Some(crate::host::dock::PaneFloat {
                pane: id,
                layers: &ws.batches.float,
                keys: host.popup(id).is_some(),
                dialog: host.dialog(id).is_some(),
            })
        })
        .collect();
    layout.picture(&host.theme, &floats)
}

/// A window's declared height, in logical pixels, at the shell's scale.
fn px(logical: u32, scale: f32) -> u32 {
    (logical as f32 * scale).round().max(1.0) as u32
}

/// The platform's cursor for [`Dock::resize_cursor`]: a resize arrow
/// sideways or up and down, the arrow off a divider.
fn resize_icon(cursor: Option<bool>) -> winit::window::CursorIcon {
    match cursor {
        Some(true) => winit::window::CursorIcon::ColResize,
        Some(false) => winit::window::CursorIcon::RowResize,
        None => winit::window::CursorIcon::Default,
    }
}
