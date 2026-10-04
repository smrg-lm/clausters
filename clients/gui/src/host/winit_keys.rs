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
/// or `None` for one the host has no name for (it then does nothing).
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
        Key::Named(NamedKey::Escape) => Some(HostKey::Escape),
        Key::Named(named) => function_key(*named).map(HostKey::F),
        Key::Character(s) => s
            .chars()
            .next()
            .filter(|c| !c.is_control())
            .map(HostKey::Char),
        _ => None,
    }
}

/// **What the host calls Ctrl**, from the two keys a keyboard has beside
/// Shift and Alt: `(ctrl, unnamed)`.
///
/// On a Mac it is **Command**, the way every program there reads a portable
/// chord -- the key table's `Ctrl+Z` is Cmd+Z -- and Control is the modifier
/// the host has no name for. Elsewhere it is Control, and the logo key is the
/// unnamed one. A key held with the unnamed modifier is no chord of the host's
/// (the fronts drop it), so Control+E on a Mac or Super+E on a desktop never
/// reaches `split` as a bare `e`.
pub(crate) fn command(control: bool, logo: bool, mac: bool) -> (bool, bool) {
    if mac {
        (logo, control)
    } else {
        (control, logo)
    }
}

/// The number of a function key, `F1` to `F12` -- the ones a binding may name.
fn function_key(key: NamedKey) -> Option<u8> {
    use NamedKey::*;
    let n = match key {
        F1 => 1,
        F2 => 2,
        F3 => 3,
        F4 => 4,
        F5 => 5,
        F6 => 6,
        F7 => 7,
        F8 => 8,
        F9 => 9,
        F10 => 10,
        F11 => 11,
        F12 => 12,
        _ => return None,
    };
    Some(n)
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
    /// a session that had just been given readers. Both spellings are one key
    /// here, which is the only place either is read.
    #[test]
    fn the_space_bar_is_recognized_by_both_spellings() {
        let space = Some(HostKey::Char(' '));
        assert_eq!(to_key(&Key::Named(NamedKey::Space)), space);
        assert_eq!(
            to_key(&Key::Character(" ".into())),
            space,
            "what a press produces"
        );
        assert_eq!(to_key(&Key::Named(NamedKey::F5)), Some(HostKey::F(5)));
        assert_eq!(to_key(&Key::Named(NamedKey::F13)), None);
    }

    /// **Ctrl is Command on a Mac**, and the key a platform leaves over is
    /// one the host has no name for.
    #[test]
    fn ctrl_is_the_platforms_command_key() {
        assert_eq!(command(false, true, true), (true, false), "Cmd on a Mac");
        assert_eq!(
            command(true, false, true),
            (false, true),
            "Control on a Mac"
        );
        assert_eq!(command(true, false, false), (true, false), "Ctrl elsewhere");
        assert_eq!(
            command(false, true, false),
            (false, true),
            "the logo key elsewhere"
        );
    }
}
