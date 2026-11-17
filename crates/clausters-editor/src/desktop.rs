//! **What the desktop answers the editor**: the file dialogs, the
//! platform's own (the desktop portal natively), and the light or dark
//! preference the editor follows when its theme is the system's. Each runs
//! on a thread of its own and tells the application what came back.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use crate::app::Msg;

/// What a file dialog was opened for, handed back with what was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A file to open in a tab.
    Open,
    /// Where to save the tab of window `win`.
    SaveAs { win: i32 },
    /// The documentation folder.
    DocsFolder,
}

/// The file types the dialogs offer.
fn python(dialog: rfd::AsyncFileDialog) -> rfd::AsyncFileDialog {
    dialog
        .add_filter("Python", &["py"])
        .add_filter("All files", &["*"])
}

/// **Asks the desktop for a path**, for `purpose`; the answer arrives as
/// `Msg::Chosen`, `None` when the dialog was cancelled.
pub fn choose(tx: &Sender<Msg>, purpose: Purpose, start: Option<PathBuf>) {
    let tx = tx.clone();
    thread::spawn(move || {
        let mut dialog = rfd::AsyncFileDialog::new();
        let dir = start.as_ref().and_then(|p| {
            if p.is_dir() {
                Some(p.clone())
            } else {
                p.parent().map(PathBuf::from)
            }
        });
        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        let path = pollster::block_on(async {
            match &purpose {
                Purpose::Open => python(dialog).pick_file().await,
                Purpose::SaveAs { .. } => {
                    let name = start
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    python(dialog).set_file_name(name).save_file().await
                }
                Purpose::DocsFolder => {
                    dialog
                        .set_title("Documentation folder (.md)")
                        .pick_folder()
                        .await
                }
            }
        })
        .map(|handle| handle.path().to_path_buf());
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
