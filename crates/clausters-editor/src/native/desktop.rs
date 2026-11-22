//! **What the desktop answers the editor**: the folder its documentation
//! is read from, in the desktop's own dialog (the portal), and the light or
//! dark preference the editor follows when its theme is the system's. Each runs
//! on a thread of its own and tells the application what came back.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use crate::app::Msg;

pub use crate::platform::Purpose;

/// **Asks the desktop for the documentation folder**, for `purpose`; the
/// answer arrives as `Msg::Chosen`, `None` when the dialog was cancelled.
/// A file is opened and saved in the editor's own dialog, so nothing else
/// is asked of the desktop.
pub fn choose(tx: &Sender<Msg>, purpose: Purpose, start: Option<PathBuf>) {
    let tx = tx.clone();
    thread::spawn(move || {
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(dir) = start.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        let path = match purpose {
            Purpose::DocsFolder => {
                pollster::block_on(dialog.set_title("Documentation folder (.md)").pick_folder())
                    .map(|handle| handle.path().to_path_buf())
            }
            Purpose::Open | Purpose::SaveAs { .. } => None,
        };
        let _ = tx.send(Msg::Chosen { purpose, path });
    });
}

/// **Follows the desktop's light or dark preference**: what it is now, then
/// every change, as `Msg::SystemDark`. A desktop that states no preference
/// is light, as a page reads it, and one with no settings portal says
/// nothing, which leaves the editor light.
pub fn follow_color_scheme(tx: &Sender<Msg>) {
    let tx = tx.clone();
    thread::spawn(move || {
        use ashpd::desktop::settings::{ColorScheme, Settings};
        use futures_util::StreamExt;
        let dark = |scheme: ColorScheme| scheme == ColorScheme::PreferDark;
        pollster::block_on(async {
            let Ok(settings) = Settings::new().await else {
                return;
            };
            if let Ok(scheme) = settings.color_scheme().await
                && tx.send(Msg::SystemDark(dark(scheme))).is_err()
            {
                return;
            }
            let Ok(mut changes) = settings.receive_color_scheme_changed().await else {
                return;
            };
            while let Some(scheme) = changes.next().await {
                if tx.send(Msg::SystemDark(dark(scheme))).is_err() {
                    return;
                }
            }
        });
    });
}
