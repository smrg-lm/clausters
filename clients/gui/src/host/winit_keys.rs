//! **The key a winit shell read, in the host's words.** Both fronts are winit
//! shells -- a window, and a canvas in a page -- so a key reaches either as the
//! same `winit::keyboard::Key`, and what it means to the host is one mapping.
//! It was written twice, once per front, and a key that did one thing in a
//! window and another in a page is the kind of difference the host being one
//! program exists to rule out.
//!
//! How a press becomes a `Key` is the shell's (a native one reads the chord's
//! unmodified key, a page reads what the browser composed); from there on it is
//! this.

use winit::keyboard::{Key, NamedKey};

use super::widget::element::Key as HostKey;

/// Translates a winit key into the platform-neutral [`HostKey`] the focus reads,
/// or `None` for one nothing focusable answers (the global shortcuts then run).
/// A printable character (including Space) inserts; the named editing keys and
/// Tab map one-to-one.
pub(crate) fn to_key(key: &Key) -> Option<HostKey> {
    match key {
        Key::Named(NamedKey::Backspace) => Some(HostKey::Backspace),
        Key::Named(NamedKey::Delete) => Some(HostKey::Delete),
        Key::Named(NamedKey::ArrowLeft) => Some(HostKey::Left),
        Key::Named(NamedKey::ArrowRight) => Some(HostKey::Right),
        Key::Named(NamedKey::ArrowUp) => Some(HostKey::Up),
        Key::Named(NamedKey::ArrowDown) => Some(HostKey::Down),
        Key::Named(NamedKey::Home) => Some(HostKey::Home),
        Key::Named(NamedKey::End) => Some(HostKey::End),
        Key::Named(NamedKey::Enter) => Some(HostKey::Enter),
        Key::Named(NamedKey::Space) => Some(HostKey::Char(' ')),
        Key::Named(NamedKey::Tab) => Some(HostKey::Tab),
        Key::Character(s) => s
            .chars()
            .next()
            .filter(|c| !c.is_control())
            .map(HostKey::Char),
        _ => None,
    }
}

/// Whether this is the space bar, however the shell spelled it.
///
/// It arrives as `Named(Space)` under a chord and as the character it typed
/// otherwise, and a match on one of the two is a key that works only with a
/// modifier held -- which is to say not at all.
pub(crate) fn is_space(key: &Key) -> bool {
    match key {
        Key::Named(NamedKey::Space) => true,
        Key::Character(c) => c == " ",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The space bar arrives as the character it typed**, not as
    /// `NamedKey::Space`, whenever no chord is held -- which is every time
    /// anybody presses it to play something.
    ///
    /// The window's arm matched only the named spelling, so the key did
    /// nothing at all: no monitor, no transport, and nothing in the log to say
    /// a key had been declined. Found 2026-09-08 by the user, pressing space at
    /// a session that had just been given readers.
    #[test]
    fn the_space_bar_is_recognized_by_both_spellings() {
        assert!(is_space(&Key::Named(NamedKey::Space)));
        assert!(
            is_space(&Key::Character(" ".into())),
            "what a press produces"
        );
        assert!(!is_space(&Key::Character("s".into())));
        assert!(!is_space(&Key::Named(NamedKey::Enter)));
    }
}
