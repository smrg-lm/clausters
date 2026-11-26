//! **What the desktop answers the editor**: the light or dark preference
//! the editor follows when its theme is the system's, asked on a thread of
//! its own, which tells the application what came back. A file and a folder
//! are chosen in the host's own dialog, so nothing else is asked of the
//! desktop.

use std::sync::mpsc::Sender;
use std::thread;

use crate::app::Msg;

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
