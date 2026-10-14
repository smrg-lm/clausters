//! X11 parent windows and bounded main-thread callback registrations.
use crate::format::EditorState;
use std::ffi::CString;
use std::time::{Duration, Instant};
use x11_dl::xlib;

pub(crate) const CALLBACKS: usize = 64;
pub(crate) struct Window {
    x: xlib::Xlib,
    display: *mut xlib::Display,
    pub id: u64,
    delete: xlib::Atom,
    state: EditorState,
    resize_pending_until: Option<Instant>,
    resizable: (bool, bool),
}
pub(crate) enum WindowEvent {
    Close,
    Resize(u32, u32),
}
impl Window {
    pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
        if !EditorState::valid_size(width, height) {
            return Err("invalid plugin editor size".into());
        }
        let title = CString::new(title).map_err(|_| "invalid plugin editor title")?;
        let x = xlib::Xlib::open().map_err(|e| e.to_string())?;
        // SAFETY: all Xlib objects below belong to this helper main thread.
        unsafe {
            let display = (x.XOpenDisplay)(std::ptr::null());
            if display.is_null() {
                return Err("native plugin editor requires an X11/XWayland display".into());
            }
            let root = (x.XDefaultRootWindow)(display);
            let id = (x.XCreateSimpleWindow)(display, root, 0, 0, width, height, 0, 0, 0);
            (x.XStoreName)(display, id, title.as_ptr());
            (x.XSelectInput)(display, id, xlib::StructureNotifyMask);
            let delete = (x.XInternAtom)(display, c"WM_DELETE_WINDOW".as_ptr(), 0);
            (x.XSetWMProtocols)(display, id, &delete as *const _ as *mut _, 1);
            Ok(Self {
                x,
                display,
                id,
                delete,
                resize_pending_until: None,
                resizable: (true, true),
                state: EditorState {
                    open: true,
                    width,
                    height,
                },
            })
        }
    }
    pub fn resize_hints(&mut self, horizontal: bool, vertical: bool) {
        self.resizable = (horizontal, vertical);
        // SAFETY: hints and all Xlib objects remain on their owning main thread.
        unsafe {
            let mut hints: xlib::XSizeHints = std::mem::zeroed();
            hints.flags = xlib::PMinSize | xlib::PMaxSize;
            hints.min_width = if horizontal {
                1
            } else {
                self.state.width as i32
            };
            hints.max_width = if horizontal {
                16384
            } else {
                self.state.width as i32
            };
            hints.min_height = if vertical {
                1
            } else {
                self.state.height as i32
            };
            hints.max_height = if vertical {
                16384
            } else {
                self.state.height as i32
            };
            (self.x.XSetWMNormalHints)(self.display, self.id, &mut hints);
        }
    }
    pub fn show(&self) {
        // SAFETY: live main-thread window and display.
        unsafe {
            (self.x.XMapRaised)(self.display, self.id);
            (self.x.XFlush)(self.display);
        }
    }
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if !EditorState::valid_size(width, height) {
            return Err("invalid plugin editor size".into());
        }
        self.resize_pending_until = Some(Instant::now() + Duration::from_millis(500));
        self.state.width = width;
        self.state.height = height;
        self.resize_hints(self.resizable.0, self.resizable.1);
        // SAFETY: live main-thread window and display.
        unsafe {
            (self.x.XResizeWindow)(self.display, self.id, width, height);
            (self.x.XFlush)(self.display);
        }
        Ok(())
    }
    pub fn state(&self) -> EditorState {
        self.state
    }
    pub fn events(&mut self) -> Vec<WindowEvent> {
        let mut events = Vec::new();
        // SAFETY: XPending makes XNextEvent nonblocking; union access follows its tag.
        unsafe {
            for _ in 0..256 {
                if (self.x.XPending)(self.display) == 0 {
                    break;
                }
                let mut event: xlib::XEvent = std::mem::zeroed();
                (self.x.XNextEvent)(self.display, &mut event);
                match event.get_type() {
                    xlib::ClientMessage
                        if event.client_message.data.get_long(0) as u64 == self.delete =>
                    {
                        events.push(WindowEvent::Close)
                    }
                    xlib::ConfigureNotify => {}
                    _ => {}
                }
            }
            // Queued ConfigureNotify events may describe an older resize.
            // Read current geometry once, instead of replaying stale sizes back
            // into the plugin and oscillating between old and new dimensions.
            let mut attributes: xlib::XWindowAttributes = std::mem::zeroed();
            if (self.x.XGetWindowAttributes)(self.display, self.id, &mut attributes) != 0 {
                let (width, height) = (attributes.width as u32, attributes.height as u32);
                if width == self.state.width && height == self.state.height {
                    self.resize_pending_until = None;
                }
                let awaiting_wm = self
                    .resize_pending_until
                    .is_some_and(|until| Instant::now() < until);
                if !awaiting_wm
                    && EditorState::valid_size(width, height)
                    && (width != self.state.width || height != self.state.height)
                {
                    self.state.width = width;
                    self.state.height = height;
                    events.push(WindowEvent::Resize(width, height));
                }
            }
        }
        events
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: plugin detaches before this parent is freed, on the same thread.
        unsafe {
            (self.x.XDestroyWindow)(self.display, self.id);
            (self.x.XCloseDisplay)(self.display);
        }
    }
}

pub(crate) struct Timer<T> {
    pub id: T,
    period: Duration,
    next: Instant,
}
impl<T> Timer<T> {
    pub fn new(id: T, milliseconds: u64) -> Result<Self, String> {
        if milliseconds == 0 {
            return Err("invalid plugin timer period".into());
        }
        let period = Duration::from_millis(milliseconds);
        let next = Instant::now()
            .checked_add(period)
            .ok_or("invalid plugin timer period")?;
        Ok(Self { id, period, next })
    }
    pub fn due(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next {
            return false;
        }
        // Coalesce missed ticks: callbacks must not starve administrative requests.
        self.next = now + self.period;
        true
    }
}
pub(crate) fn fd_ready(fd: i32, events: i16) -> i16 {
    let mut descriptor = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: one valid descriptor, zero timeout, main thread only.
    if unsafe { libc::poll(&mut descriptor, 1, 0) } > 0 {
        descriptor.revents
    } else {
        0
    }
}
