//! The theme: every chrome color as a named **role**, in one table.
//!
//! No paint site names an RGBA literal — it names a role of this struct, so
//! the host has exactly one look, replaceable as a whole. The default theme
//! reproduces the colors the widgets always had; a partial overlay (the
//! `[gui.theme]` config table, a `--theme` TOML file, or — later — a theme
//! group on a container) renames only the roles it lists, in `"#rrggbb[aa]"`
//! form. Roles are colors and only colors: a widget's parts are named by
//! function here, never addressed individually, and anything particular to
//! one visualization (the spectrogram's colormap) stays that widget's own
//! prop.
//!
//! Where one rgb serves several alphas (a selection's fill vs. its edge), the
//! role stores the opaque color and the paint site derives with
//! [`with_alpha`] — multiplied, so a translucent themed role stays
//! translucent.

use super::paint::Color;

/// `color` with its alpha multiplied by `a` — the one sanctioned derivation
/// at a paint site (fills vs. edges of one role).
pub fn with_alpha(color: Color, a: f32) -> Color {
    [color[0], color[1], color[2], color[3] * a]
}

macro_rules! theme_roles {
    ($( $(#[$doc:meta])* $name:ident = [$r:expr, $g:expr, $b:expr, $a:expr]; )+) => {
        /// The host's color roles. One instance per host; every paint site
        /// reads one role.
        #[derive(Debug, Clone, PartialEq)]
        pub struct Theme {
            $( $(#[$doc])* pub $name: Color, )+
        }

        impl Default for Theme {
            fn default() -> Self {
                Self { $( $name: [$r, $g, $b, $a], )+ }
            }
        }

        impl Theme {
            /// Every role name, in declaration order.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)+];

            /// Sets one role by name. `false` if no such role exists.
            pub fn set(&mut self, name: &str, color: Color) -> bool {
                match name {
                    $( stringify!($name) => { self.$name = color; true } )+
                    _ => false,
                }
            }

            /// Reads one role by name.
            pub fn get(&self, name: &str) -> Option<Color> {
                match name {
                    $( stringify!($name) => Some(self.$name), )+
                    _ => None,
                }
            }
        }
    };
}

theme_roles! {
    // -- Window chrome --
    /// The window clear color, the backdrop under everything.
    background = [0.05, 0.05, 0.07, 1.0];
    /// A panel's translucent fill over the backdrop.
    panel = [0.10, 0.11, 0.14, 0.55];
    /// Primary text: labels, values, readouts.
    text = [0.85, 0.87, 0.90, 1.0];
    /// De-emphasized text (the node tree's parameter lines).
    text_dim = [0.55, 0.60, 0.66, 1.0];
    /// De-emphasized in-view labels (a lane's name tag).
    label_dim = [0.60, 0.63, 0.70, 1.0];

    // -- Controls --
    /// A control's body fill (slider, knob, scope field).
    field = [0.14, 0.15, 0.19, 1.0];
    /// The inset groove/body under a control's value (a slider's track), and
    /// the body of the static info views (node tree, plot).
    track = [0.10, 0.11, 0.14, 1.0];
    /// The color that carries a widget's function: a slider's fill, a knob's
    /// pointer, a meter's bar, the live views' frame.
    accent = [0.30, 0.78, 0.55, 1.0];
    /// The accent's quiet form (an unlit toggle, a knob's arc).
    accent_dim = [0.22, 0.50, 0.40, 1.0];
    /// The accent's lit form (a pressed control, a window's edge marker).
    hilite = [0.40, 0.85, 0.62, 1.0];

    // -- Data traces --
    /// A drawn signal or curve (scope trace, bpf curve, automation curve).
    trace = [0.40, 0.85, 0.62, 1.0];
    /// The brighter trace of the phase scope's beam.
    trace_bright = [0.45, 0.90, 0.66, 1.0];
    /// A curve's grabbable break-point.
    point = [0.90, 0.93, 0.95, 1.0];

    // -- Frames --
    /// The neutral frame of the editor views (piano roll, tracks, patch).
    frame = [0.30, 0.34, 0.42, 1.0];
    /// The frame of the timeline views (waveform, spectrogram).
    view_frame = [0.25, 0.45, 0.38, 1.0];
    /// The frame of the live info view (the node tree).
    frame_info = [0.30, 0.45, 0.60, 1.0];
    /// The frame of the measuring plot.
    frame_plot = [0.45, 0.55, 0.70, 1.0];

    // -- View fields, lanes and grids --
    /// The dark body of the heavy views (waveform, spectrogram, patch).
    view_field = [0.08, 0.09, 0.11, 1.0];
    /// A lane's background (a track lane, the piano roll's grid).
    lane = [0.09, 0.10, 0.13, 1.0];
    /// A track's header strip.
    header = [0.14, 0.16, 0.20, 1.0];
    /// The alternate, darker lane (black-key rows, the velocity lane).
    lane_alt = [0.07, 0.08, 0.10, 1.0];
    /// The divider line between stacked lanes.
    lane_divider = [0.30, 0.33, 0.38, 0.8];
    /// A view's reference grid (the phase scope's cross and square).
    grid = [0.30, 0.34, 0.40, 0.6];
    /// A fine grid line (the piano roll's row lines).
    grid_line = [0.16, 0.18, 0.22, 1.0];
    /// The zero baseline of a value axis.
    baseline = [0.28, 0.32, 0.38, 1.0];

    // -- Navigation chrome --
    /// The selection color; fills and edges derive by alpha.
    selection = [0.55, 0.75, 0.95, 1.0];
    /// The playhead line.
    playhead = [0.95, 0.55, 0.30, 0.9];
    /// Ruler tick labels.
    ruler_text = [0.65, 0.68, 0.72, 1.0];
    /// Ruler tick lines.
    ruler_line = [0.45, 0.48, 0.52, 1.0];

    // -- Placed objects (clips, patch boxes) --
    /// A placed object's fill (a clip's body, a patch member box).
    object_fill = [0.16, 0.22, 0.32, 1.0];
    /// A placed object's edge.
    object_edge = [0.45, 0.60, 0.85, 1.0];
    /// A patch bus node's fill.
    bus_fill = [0.18, 0.28, 0.24, 1.0];
    /// A patch box's wiring port.
    port = [0.75, 0.82, 0.92, 1.0];
    /// The live/rendered marker (a sounding patch wire).
    live = [0.95, 0.72, 0.25, 1.0];

    // -- Notes and events --
    /// A note's fill in the piano roll.
    note_fill = [0.55, 0.80, 0.62, 1.0];
    /// A note's edge.
    note_edge = [0.78, 0.95, 0.82, 1.0];
    /// A selected note's fill.
    selected_fill = [0.80, 0.90, 0.98, 1.0];
    /// A selected note's edge.
    selected_edge = [1.0, 1.0, 1.0, 1.0];
    /// A velocity bar.
    velocity = [0.70, 0.55, 0.90, 1.0];
    /// The OSC event lane's background.
    event_lane = [0.10, 0.09, 0.13, 1.0];
    /// An event marker flag (an OSC marker, an overview's pressed key).
    flag = [0.95, 0.75, 0.45, 1.0];
    /// The oscilloscope's trigger-level line.
    trigger = [0.85, 0.80, 0.40, 0.4];
    /// The negative/warning readout (the phase scope's anti-correlation).
    warn = [0.85, 0.42, 0.42, 1.0];

    // -- The keyboard --
    /// A playable white key.
    key_white = [0.86, 0.87, 0.90, 1.0];
    /// The piano roll's dimmer white key.
    key_white_dim = [0.82, 0.84, 0.88, 1.0];
    /// A black key.
    key_black = [0.10, 0.11, 0.14, 1.0];
    /// A pressed white key.
    key_pressed = [0.45, 0.80, 0.60, 1.0];
    /// A pressed black key.
    key_pressed_black = [0.25, 0.60, 0.42, 1.0];
    /// An inactive (out-of-range) white key.
    key_inactive = [0.42, 0.43, 0.46, 1.0];
    /// An inactive black key.
    key_inactive_black = [0.24, 0.25, 0.28, 1.0];
    /// The gap/edge line between keys.
    key_gap = [0.05, 0.06, 0.08, 1.0];
    /// The octave label on a playable key.
    key_label = [0.35, 0.37, 0.42, 1.0];
    /// The octave label in the piano roll's key gutter.
    key_label_dim = [0.30, 0.32, 0.38, 1.0];
    /// The keyboard overview strip's background.
    key_overview = [0.07, 0.08, 0.10, 1.0];
    /// The overview's active-range band.
    key_overview_active = [0.16, 0.18, 0.22, 1.0];
    /// The overview's black-key marks.
    key_overview_black = [0.04, 0.05, 0.06, 1.0];

    // -- The series palette (multichannel traces, cycled) --
    /// Channel 1 of the series palette (the classic mono trace).
    series_1 = [0.30, 0.78, 0.55, 1.0];
    /// Channel 2 of the series palette.
    series_2 = [0.95, 0.72, 0.25, 1.0];
    /// Channel 3 of the series palette.
    series_3 = [0.45, 0.65, 0.95, 1.0];
    /// Channel 4 of the series palette.
    series_4 = [0.90, 0.45, 0.60, 1.0];
}

impl Theme {
    /// The series palette color for channel `ch`, cycled.
    pub fn series(&self, ch: usize) -> Color {
        [self.series_1, self.series_2, self.series_3, self.series_4][ch % 4]
    }

    /// Overlays `(role, "#rrggbb[aa]")` pairs onto this theme. Unknown roles
    /// and unparsable colors are skipped and reported back as warnings, so a
    /// stale theme file degrades to the default look, never to an error.
    pub fn overlay<'a, I>(&mut self, entries: I) -> Vec<String>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut warnings = Vec::new();
        for (role, hex) in entries {
            match parse_hex(hex) {
                Some(color) => {
                    if !self.set(role, color) {
                        warnings.push(format!("theme: unknown role '{role}'"));
                    }
                }
                None => warnings.push(format!("theme: bad color '{hex}' for role '{role}'")),
            }
        }
        warnings
    }
}

/// Parses `"#rrggbb"` / `"#rrggbbaa"` (the `#` optional) into a [`Color`].
pub fn parse_hex(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let (r, g, b) = (byte(0)?, byte(2)?, byte(4)?);
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ])
}

/// Formats a [`Color`] back to `"#rrggbb"` or `"#rrggbbaa"`.
pub fn to_hex(c: Color) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if c[3] >= 1.0 {
        format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            b(c[0]),
            b(c[1]),
            b(c[2]),
            b(c[3])
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        for hex in ["#000000", "#ffffff", "#4dc78c", "#8cbff2bf"] {
            assert_eq!(to_hex(parse_hex(hex).unwrap()), hex);
        }
        assert_eq!(parse_hex("4dc78c"), parse_hex("#4dc78c"));
    }

    #[test]
    fn hex_rejects_malformed() {
        for bad in ["", "#fff", "#12345", "#1234567", "#gggggg", "#123456789"] {
            assert!(parse_hex(bad).is_none(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn partial_overlay_keeps_the_rest() {
        let mut theme = Theme::default();
        let warnings = theme.overlay([("accent", "#ff0000")]);
        assert!(warnings.is_empty());
        assert_eq!(theme.accent, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            theme.text,
            Theme::default().text,
            "unlisted roles keep the default"
        );
    }

    #[test]
    fn overlay_warns_and_continues() {
        let mut theme = Theme::default();
        let warnings = theme.overlay([
            ("no_such_role", "#112233"),
            ("text", "nonsense"),
            ("field", "#101010"),
        ]);
        assert_eq!(warnings.len(), 2);
        assert_eq!(
            theme.text,
            Theme::default().text,
            "a bad color leaves the role"
        );
        assert_eq!(
            theme.field,
            parse_hex("#101010").unwrap(),
            "later entries still apply"
        );
    }

    #[test]
    fn every_name_sets_and_gets() {
        let mut theme = Theme::default();
        for name in Theme::NAMES {
            assert!(theme.get(name).is_some());
            assert!(theme.set(name, [0.1, 0.2, 0.3, 1.0]));
        }
    }

    #[test]
    fn series_cycles() {
        let theme = Theme::default();
        assert_eq!(theme.series(0), theme.series_1);
        assert_eq!(theme.series(5), theme.series_2);
    }

    #[test]
    fn with_alpha_multiplies() {
        assert_eq!(with_alpha([0.2, 0.4, 0.6, 0.5], 0.5), [0.2, 0.4, 0.6, 0.25]);
    }
}
