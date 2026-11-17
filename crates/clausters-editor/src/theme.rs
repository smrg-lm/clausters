//! **The editor's two looks**: the host's color roles in the colors the
//! webview application draws with (its neutral palette, light and dark),
//! handed to the host as `/gui_theme` tables. What a role is is the host's;
//! these say only which of the webview's colors each one takes.

use serde_json::{Value, json};

/// The light look.
pub fn light() -> Value {
    json!({
        "background": "#ffffff",
        "panel": "#f5f5f5",
        "status_bar": "#f5f5f5",
        "header": "#f5f5f5",
        "field": "#ffffff",
        "view_field": "#ffffff",
        "track": "#e5e5e5",
        "popup": "#ffffff",
        "text": "#0a0a0a",
        "text_dim": "#737373",
        "label_dim": "#737373",
        "ruler_text": "#737373",
        "text_disabled": "#a1a1a1",
        "hover": "#ececec",
        "separator": "#e5e5e5",
        "frame": "#e5e5e5",
        "grid_line": "#ececec",
        "focus": "#a1a1a1",
        "selection": "#3b82f6",
        "cursor": "#171717",
        "accent": "#16a34a",
        "accent_dim": "#e5e5e5",
        "hilite": "#16a34a",
        "warn": "#e7000b",
    })
}

/// The dark look.
pub fn dark() -> Value {
    json!({
        "background": "#1e1e1e",
        "panel": "#262626",
        "status_bar": "#262626",
        "header": "#262626",
        "field": "#171717",
        "view_field": "#1e1e1e",
        "track": "#404040",
        "popup": "#171717",
        "text": "#fafafa",
        "text_dim": "#a1a1a1",
        "label_dim": "#a1a1a1",
        "ruler_text": "#a1a1a1",
        "text_disabled": "#737373",
        "hover": "#2e2e2e",
        "separator": "#ffffff1a",
        "frame": "#ffffff26",
        "grid_line": "#2a2a2a",
        "focus": "#737373",
        "selection": "#60a5fa",
        "cursor": "#e5e5e5",
        "accent": "#4ade80",
        "accent_dim": "#404040",
        "hilite": "#4ade80",
        "warn": "#ff6467",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both looks name the same roles, so switching one for the other leaves
    /// nothing of the first behind, and every role is one the host knows.
    #[test]
    fn both_looks_name_the_same_known_roles() {
        let (l, d) = (light(), dark());
        let names = |v: &Value| {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(names(&l), names(&d));
        for look in [l, d] {
            let mut theme = clausters_gui_draw::theme::Theme::default();
            let table: Vec<(String, String)> = look
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
            let warnings = theme.overlay(table.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            assert!(warnings.is_empty(), "{warnings:?}");
        }
    }
}
