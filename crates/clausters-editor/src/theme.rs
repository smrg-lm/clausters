//! **The editor's two looks**: the host's own, dark and light, which are the
//! specification's the webview application draws with. The editor names
//! which one, as a `/gui_theme` table holding only the reserved `look` key;
//! every role is the host's.

use serde_json::{Value, json};

/// The light look.
pub fn light() -> Value {
    json!({"look": "light"})
}

/// The dark look.
pub fn dark() -> Value {
    json!({"look": "dark"})
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_gui_draw::theme::Theme;

    /// Each look is one the host knows, and switching from one to the other
    /// leaves nothing of the first behind.
    #[test]
    fn each_look_is_the_host_s() {
        let overlaid = |look: &Value| {
            let mut theme = Theme::default();
            theme.overlay([("selection", "#123456")]);
            let table = look.as_object().unwrap();
            let warnings = theme.overlay_json(table);
            assert!(warnings.is_empty(), "{warnings:?}");
            theme
        };
        assert_eq!(overlaid(&light()), Theme::light());
        assert_eq!(overlaid(&dark()), Theme::dark());
    }
}
