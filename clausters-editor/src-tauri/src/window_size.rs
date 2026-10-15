//! The window's size on Linux, kept and restored by the editor itself.
//!
//! The window-state plugin keeps the rest (maximized, position, ...), but on
//! Linux it cannot keep the size: it reads the size of GTK's configure event,
//! which counts the shadow and title bar a client-side decoration draws, and it
//! restores with `gtk_window_resize`, which does not. So every launch grew the
//! window by the decoration's size until the screen stopped it. GTK's own pair
//! for this, `gtk_window_get_size`, answers exactly what `gtk_window_resize`
//! takes, so the size read here comes back as it was left.

use std::path::PathBuf;

use gtk::prelude::GtkWindowExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewWindow, WindowEvent};

const FILE: &str = ".window-size.json";

/// In GTK's logical pixels, as `gtk_window_get_size` gives them.
#[derive(Serialize, Deserialize)]
struct Size {
    width: i32,
    height: i32,
}

fn path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(FILE))
}

/// Gives `window` the size it was left at, and keeps the one it is closed at.
pub fn manage(window: &WebviewWindow) {
    let app = window.app_handle().clone();
    if let Some(size) = path(&app)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Size>(&bytes).ok())
        .filter(|s| s.width > 0 && s.height > 0)
    {
        if let Ok(gtk) = window.gtk_window() {
            gtk.resize(size.width, size.height);
        }
    }
    let window = window.clone();
    window.clone().on_window_event(move |event| {
        if let WindowEvent::CloseRequested { .. } = event {
            save(&app, &window);
        }
    });
}

/// The size the window has unmaximized; a maximized or minimized one keeps
/// the size saved before, which is the one it goes back to.
fn save(app: &AppHandle, window: &WebviewWindow) {
    if window.is_maximized().unwrap_or(true) || window.is_minimized().unwrap_or(true) {
        return;
    }
    let (Some(path), Ok(gtk)) = (path(app), window.gtk_window()) else {
        return;
    };
    let (width, height) = gtk.size();
    if let Ok(json) = serde_json::to_vec(&Size { width, height }) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, json);
    }
}
