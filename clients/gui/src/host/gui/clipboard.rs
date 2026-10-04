//! **The desktop's clipboard**, for the windowed front: the
//! [`SystemClipboard`] the host's typed clipboard reaches it through. Text in
//! and out, and the primary selection on a Linux desktop (X11 and Wayland),
//! which is what a middle click pastes there.
//!
//! Every failure is quiet -- a clipboard nobody owns, a compositor that offers
//! no data-control protocol -- since the host then keeps working with its own.

use crate::host::clipboard::SystemClipboard;

/// The platform's clipboard, kept open for the life of the front: on X11 the
/// text a host copied is served by this object, so it has to outlive the copy.
pub(super) struct Desktop(arboard::Clipboard);

impl Desktop {
    /// The desktop's clipboard, or `None` where there is none to open (a
    /// headless session), which leaves the host its own.
    pub(super) fn open() -> Option<Box<dyn SystemClipboard>> {
        match arboard::Clipboard::new() {
            Ok(c) => Some(Box::new(Desktop(c))),
            Err(e) => {
                tracing::warn!("no system clipboard ({e}); copies stay inside the host");
                None
            }
        }
    }
}

impl SystemClipboard for Desktop {
    fn read_text(&mut self) -> Option<String> {
        self.0.get_text().ok()
    }

    fn write_text(&mut self, text: &str) {
        if let Err(e) = self.0.set_text(text) {
            tracing::debug!("system clipboard: {e}");
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn read_primary(&mut self) -> Option<String> {
        use arboard::{GetExtLinux, LinuxClipboardKind};
        self.0
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
            .ok()
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn write_primary(&mut self, text: &str) {
        use arboard::{LinuxClipboardKind, SetExtLinux};
        if let Err(e) = self
            .0
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text.to_string())
        {
            tracing::debug!("primary selection: {e}");
        }
    }
}
