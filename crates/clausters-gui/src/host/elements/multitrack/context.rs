//! **The context menu**: what the secondary button opens over the stack, and
//! the commands its entries name.
//!
//! What a menu offers here is what has no gesture of its own: which of a
//! clip's or a track's curves are shown, one at a time -- the `A` of a header
//! shows or hides them all at once -- and how tall the rows are as a whole.
//! A box is moved, trimmed and faded by the hand, and those are not here.
//!
//! An entry's verb carries its subject (`show:<curve>:<0|1>`, `add_track:<at>`),
//! so a pick needs nothing remembered of where the menu was opened.

use super::*;
use crate::host::menu::Entry;

/// Shows or hides one curve: `show:<name>:<1|0>`.
const SHOW: &str = "show";
/// Adds a track: `add_track:<at>`, or bare for one after the held track.
const ADD_TRACK: &str = "add_track";
/// Every row back at the height its multitrack states.
const RESET_HEIGHTS: &str = "reset_heights";
/// Every row at its floor.
const COMPACT: &str = "compact_tracks";

impl Multitrack {
    /// **The menu at `at`**: over a box, that box's curves; over a track or one
    /// of its automation rows, the track's curves and a track added under it;
    /// and everywhere, the heights of the rows as a whole.
    pub(super) fn context_at(&self, at: (f64, f64), input: &Input) -> Vec<Entry> {
        let mut out = Vec::new();
        if let Some((clip, _)) = self.clip_at(input, at) {
            let owner = &self.clips[clip].name;
            out.extend(self.curve_checks(&self.layers, owner, "No clip automation"));
            out.push(Entry::separator());
        }
        let track = match self.row_kind(input, at.1) {
            Some(stack::Row::TrackRow(i)) => Some(i),
            Some(stack::Row::Curve(n)) => self
                .curves
                .get(n)
                .and_then(|c| self.tracks.iter().position(|t| t.name == c.owner)),
            None => None,
        };
        if let Some(i) = track.filter(|_| out.is_empty()) {
            let owner = &self.tracks[i].name;
            out.extend(self.curve_checks(&self.curves, owner, "No track automation"));
            out.push(Entry::separator());
        }
        let at = track.map_or(self.tracks.len(), |i| i + 1);
        out.push(Entry::action("Add track", &format!("{ADD_TRACK}:{at}")));
        out.push(Entry::separator());
        out.push(Entry::action("Reset track heights", RESET_HEIGHTS));
        out.push(Entry::action("Compact tracks", COMPACT));
        out
    }

    /// A check per curve of `owner` in `curves`, on where it is shown -- or one
    /// line saying there are none.
    fn curve_checks(&self, curves: &[model::Curve], owner: &str, none: &str) -> Vec<Entry> {
        let checks: Vec<Entry> = curves
            .iter()
            .filter(|c| c.owner == owner)
            .map(|c| {
                let shown = !self.is_hidden(&c.name);
                let label = if c.label.is_empty() {
                    &c.name
                } else {
                    &c.label
                };
                Entry::check(
                    label,
                    &format!("{SHOW}:{}:{}", c.name, u8::from(!shown)),
                    shown,
                )
            })
            .collect();
        if checks.is_empty() {
            vec![Entry::action(none, "").disabled()]
        } else {
            checks
        }
    }

    /// **A command an entry named**: `None` for a word that is not this
    /// element's.
    pub(super) fn commanded(&mut self, name: &str) -> Option<Events> {
        let (word, rest) = name.split_once(':').unwrap_or((name, ""));
        match word {
            SHOW => {
                let (curve, on) = rest.rsplit_once(':')?;
                Some(self.show_curve(curve, on == "1"))
            }
            ADD_TRACK => {
                let at = rest
                    .parse()
                    .unwrap_or_else(|_| self.track.map_or(self.tracks.len(), |t| t + 1));
                match self.add_track(at) {
                    Claim::Take(take) => Some(take.events),
                    _ => None,
                }
            }
            RESET_HEIGHTS => {
                self.reset_heights();
                Some(Events::none())
            }
            COMPACT => {
                self.compact_heights();
                Some(Events::none())
            }
            _ => None,
        }
    }

    /// **Shows or hides one curve**: the picture moves under the hand, as the
    /// header's toggle moves it, and the owner is told in a `shown` report --
    /// `name flag` -- which it answers by saying which curves are hidden.
    fn show_curve(&mut self, curve: &str, on: bool) -> Events {
        self.hidden.retain(|h| h != curve);
        if !on {
            self.hidden.push(curve.to_string());
        }
        Events::message(vec![
            OscType::String("shown".into()),
            OscType::String(curve.to_string()),
            OscType::Int(i32::from(on)),
        ])
    }

    /// **Every row at the height its multitrack states**: a reader's zoom
    /// taken back, track and automation row alike.
    pub(super) fn reset_heights(&mut self) {
        self.zoom.clear();
        self.curve_zoom.clear();
        for track in &mut self.tracks {
            track.height = self.stated.get(&track.name).copied().unwrap_or(LANE_H);
        }
        for curve in &mut self.curves {
            curve.height = self
                .curve_stated
                .get(&curve.name)
                .copied()
                .unwrap_or(CURVE_H);
        }
        self.scroll = 0.0;
    }

    /// **Every row at its floor**, so the whole stack is in view at once --
    /// kept as a reader's zoom, which the next payload does not take away.
    pub(super) fn compact_heights(&mut self) {
        for track in &mut self.tracks {
            track.height = MIN_LANE_H;
            self.zoom.insert(track.name.clone(), MIN_LANE_H);
        }
        for curve in &mut self.curves {
            curve.height = MIN_CURVE_H;
            self.curve_zoom.insert(curve.name.clone(), MIN_CURVE_H);
        }
        self.scroll = 0.0;
    }

    /// **The heights a payload states**, read as it lands and before a
    /// reader's zoom is laid over them: what a reset goes back to.
    pub(super) fn state_heights(&mut self) {
        self.stated = self
            .tracks
            .iter()
            .map(|t| (t.name.clone(), t.height))
            .collect();
    }

    /// The same for the automation rows.
    pub(super) fn state_curve_heights(&mut self) {
        self.curve_stated = self
            .curves
            .iter()
            .map(|c| (c.name.clone(), c.height))
            .collect();
    }
}
