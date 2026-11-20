//! **The preferences and what the editor remembers between runs**: the
//! theme, the size of the text, the indentation, the post window's scroll,
//! how a block is evaluated, the recent files, the files left open, the
//! dock's arrangement, the window's size and place, and the code's font --
//! `settings.json`, kept by [`store`](crate::store).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How many recent files File > Open Recent keeps.
pub const MAX_RECENT: usize = 10;
pub const DEFAULT_SIZE: f32 = 14.0;
pub const MIN_SIZE: f32 = 9.0;
pub const MAX_SIZE: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub theme: ThemePref,
    /// The text's size, in the webview application's pixels.
    pub size: f32,
    /// The post window follows each new output.
    pub autoscroll: bool,
    /// Ctrl+Enter takes the contiguous lines around the caret as one block,
    /// rather than the one top-level statement.
    pub eval_paragraph: bool,
    /// One indentation level: `"tab"`, `"2"` or `"4"`.
    pub indent: String,
    /// The files last opened or saved, most recent first.
    pub recent: Vec<String>,
    /// At start, the files open when the editor was closed open again.
    pub reopen_files: bool,
    /// The files open now, in tab order, kept whether or not `reopen_files`
    /// is on so that turning it on finds them.
    pub open_files: Vec<String>,
    /// The one on screen.
    pub active_file: Option<String>,
    /// The dock's arrangement as it answered it, by the panels' roles.
    pub layout: Option<Value>,
    /// The window's size, in logical pixels.
    pub window: Option<(u32, u32)>,
    /// Where the window was on the screen, its outer corner in the screen's
    /// pixels, on a desktop that says it.
    pub position: Option<(i32, i32)>,
    /// The family code is written in, `None` for the one the host carries.
    pub font: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::System,
            size: DEFAULT_SIZE,
            autoscroll: true,
            eval_paragraph: false,
            indent: "4".into(),
            recent: Vec::new(),
            reopen_files: false,
            open_files: Vec::new(),
            active_file: None,
            layout: None,
            window: None,
            position: None,
            font: None,
        }
    }
}

const NAME: &str = "settings.json";

impl Settings {
    pub fn load() -> Self {
        let mut s: Settings = crate::store::read(NAME)
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        s.size = s.size.clamp(MIN_SIZE, MAX_SIZE);
        if !["tab", "2", "4"].contains(&s.indent.as_str()) {
            s.indent = "4".into();
        }
        s.recent.retain(|p| !p.is_empty());
        s.recent.truncate(MAX_RECENT);
        s
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self)
            && let Err(e) = crate::store::write(NAME, &json)
        {
            tracing::warn!("the preferences are not kept: {e}");
        }
    }

    /// `path` first in the recent files.
    pub fn add_recent(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(MAX_RECENT);
    }

    /// The code element's `indent`: a number of spaces or `"tab"`.
    pub fn indent_prop(&self) -> Value {
        match self.indent.as_str() {
            "tab" => Value::from("tab"),
            n => Value::from(n.parse::<u32>().unwrap_or(4)),
        }
    }

    /// The elements' `text_size` for the size in pixels: the glyph scale
    /// whose cap height is the face's at that size (JetBrains Mono's cap
    /// height is 0.73 of its em, over the 7-pixel cell).
    pub fn text_size(&self) -> f64 {
        f64::from(self.size) * 0.73 / 7.0
    }

    /// The size changed by `delta`; `None` goes back to the default.
    pub fn zoom(&mut self, delta: Option<f32>) {
        self.size = match delta {
            None => DEFAULT_SIZE,
            Some(d) => (self.size + d).clamp(MIN_SIZE, MAX_SIZE),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file opened again goes first once, and the list keeps ten.
    #[test]
    fn recent_files_keep_their_order() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.add_recent(&format!("/f{i}.py"));
        }
        s.add_recent("/f5.py");
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], "/f5.py");
        assert_eq!(s.recent.iter().filter(|p| *p == "/f5.py").count(), 1);
    }

    /// The default size is the elements' own default, near enough.
    #[test]
    fn the_default_size_is_the_elements() {
        let s = Settings::default();
        assert!((s.text_size() - 1.5).abs() < 0.05);
        let mut s = s;
        s.zoom(Some(100.0));
        assert_eq!(s.size, MAX_SIZE);
        s.zoom(None);
        assert_eq!(s.size, DEFAULT_SIZE);
    }
}
