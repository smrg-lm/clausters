//! **The time ruler**: one strip over every window with a time axis -- the
//! audio editor's, the multitrack's, the roll's -- written once.
//!
//! It is a free-standing `field` over the views -- or under them, drawn as
//! its mirror, where the window reads better so ([`Side`]) -- a member of the
//! window's navigation group, so it rules the axis every view under it is
//! drawn on, carries its play cursor and its position cursor, and a press or
//! a sweep on it is the window's (a locate, a time range). The views under it
//! draw no strip of their own (`ruler_strip` off) and still read their time
//! in its unit.
//!
//! **Its unit is the window's**, chosen from the View menu's Ruler or a
//! client's `ruler`: bars and beats, minutes and seconds, or samples. It is
//! window state, like the snap switch: recorded nowhere, and two windows
//! over one object may count differently.
//!
//! What an application adds is its own: a multitrack's tempo map and bars, a
//! roll's page bars, a take's rate alone. Each states it on a [`TimeRuler`],
//! the one shape, which this module turns into the strip and the props of
//! the views under it.
//!
//! **A value ruler is the same ruler on the other axis** ([`ValueRuler`]): a
//! free-standing `field` beside one view, ruling its vertical in the view's
//! own unit, on whichever side the window reads better -- the audio editor's
//! on the right of its take, beside the meter. The view under it keeps no
//! strip of its own, as the views under a time ruler keep none.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// How tall the strip is, in logical pixels.
pub const HEIGHT: f64 = 20.0;

/// **What the ruler counts in.**
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    /// Bars and beats, through the axis' tempo map.
    #[default]
    Beats,
    /// Minutes and seconds.
    Time,
    /// Samples.
    Samples,
}

/// **The verbs that choose the unit**, by unit: the View menu's Ruler.
pub const VERBS: &[(&str, Unit)] = &[
    ("ruler_beats", Unit::Beats),
    ("ruler_time", Unit::Time),
    ("ruler_samples", Unit::Samples),
];

impl Unit {
    /// The unit as the host's `ruler` prop spells it.
    #[must_use]
    pub fn prop(self) -> &'static str {
        match self {
            Unit::Beats => "beats",
            Unit::Time => "time",
            Unit::Samples => "samples",
        }
    }

    /// The unit a name says (`beats`, `time`, `samples`), as a client sets it.
    #[must_use]
    pub fn named(name: &str) -> Option<Unit> {
        VERBS
            .iter()
            .map(|(_, unit)| *unit)
            .find(|unit| unit.prop() == name)
    }

    /// The unit a verb of [`VERBS`] chooses.
    #[must_use]
    pub fn of_verb(verb: &str) -> Option<Unit> {
        VERBS
            .iter()
            .find(|(v, _)| *v == verb)
            .map(|(_, unit)| *unit)
    }

    /// Whether `verb` is the one that chooses this unit: the choice in
    /// force, for the window's actions.
    #[must_use]
    pub fn chosen_by(self, verb: &str) -> Option<bool> {
        Unit::of_verb(verb).map(|unit| unit == self)
    }

    /// The answer to a client's `ruler` set to `name`: the unit, or why not.
    ///
    /// # Errors
    ///
    /// A name no unit has.
    pub fn parse(name: &str) -> Result<Unit, String> {
        Unit::named(name)
            .ok_or_else(|| format!("a ruler counts in beats, time or samples, not {name:?}"))
    }
}

/// **Which side of the views the strip is on**, and so which way it faces:
/// its marks hug the edge the views are on, so a strip under them is the
/// mirror of one over them, and one on their right the mirror of one on
/// their left. A time ruler stands over or under; a value ruler beside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    /// Over the views, its marks along its bottom edge: the multitrack's and
    /// the roll's.
    #[default]
    Top,
    /// Under them, its marks along its top edge: the audio editor's, under
    /// its take.
    Bottom,
    /// On the view's left, its marks along its right edge: where a view's
    /// own value strip stands.
    Left,
    /// On the view's right, its marks along its left edge: the audio
    /// editor's, beside the meter.
    Right,
}

impl Side {
    /// The way a strip on this side faces the views, as the host's `dir`
    /// prop spells it.
    #[must_use]
    pub fn dir(self) -> &'static str {
        match self {
            Side::Top => "down",
            Side::Bottom => "up",
            Side::Left => "right",
            Side::Right => "left",
        }
    }
}

/// **One window's time ruler**: the strip's id, its unit, and what the
/// application adds to the axis it rules. Each application states its own
/// and the strip is made here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimeRuler {
    /// The widget id the caller numbered the strip with.
    pub id: i32,
    /// What it counts in.
    pub unit: Unit,
    /// The navigation group it rules: the views' `link`.
    pub link: i64,
    /// Which side of the views it is on.
    pub side: Side,
    /// The axis' samples a second.
    pub rate: f64,
    /// Everything else the application's axis states: a tempo map, bars, a
    /// cursor, a playhead's anchor -- the `axes.x` keys of the views it rules.
    pub axis: Map<String, Value>,
}

impl TimeRuler {
    /// **The strip**, as the window's first child.
    #[must_use]
    pub fn widget(&self) -> Value {
        let mut x = self.axis.clone();
        x.insert("unit".into(), json!(self.unit.prop()));
        x.insert("sample_rate".into(), json!(self.rate));
        x.insert("link".into(), json!(self.link));
        x.insert("dir".into(), json!(self.side.dir()));
        json!({
            "type": "field",
            "id": self.id,
            "h": HEIGHT,
            "axes": {"x": x},
        })
    }

    /// **A view under the strip**: in its unit, on its group, with no strip
    /// of its own. `props` are the view's, its axis flat or under `axes.x`.
    pub fn rule(&self, props: &mut Map<String, Value>) {
        let unit = json!(self.unit.prop());
        if let Some(Value::Object(x)) = props.get_mut("axes").and_then(|a| a.get_mut("x")) {
            x.insert("unit".into(), unit);
            x.insert("strip".into(), json!(false));
            x.insert("link".into(), json!(self.link));
            return;
        }
        props.insert("ruler".into(), unit);
        props.insert("ruler_strip".into(), json!(false));
        props.insert("link".into(), json!(self.link));
    }
}

/// **One view's value ruler**: the strip's id, the view it rules and the
/// side of it it stands on. The unit, the window and the rows are the
/// view's, which the host reads from the view itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueRuler {
    /// The widget id the caller numbered the strip with.
    pub id: i32,
    /// The widget id of the view it rules: a take, a lane, or the pane whose
    /// body in hand it rules.
    pub rules: i32,
    /// Which side of the view it stands on.
    pub side: Side,
}

impl ValueRuler {
    /// **The strip**, elastic along the view and as wide as its labels.
    #[must_use]
    pub fn widget(&self) -> Value {
        json!({
            "type": "field",
            "id": self.id,
            "weight": 1,
            "axes": {"y": {"rules": self.rules, "dir": self.side.dir()}},
        })
    }

    /// **A view beside the strip**: it keeps no value strip of its own.
    /// `props` are the view's, its vertical flat or under `axes.y`.
    pub fn rule(props: &mut Map<String, Value>) {
        let axes = props
            .entry("axes")
            .or_insert_with(|| json!({}))
            .as_object_mut();
        if let Some(axes) = axes {
            let y = axes.entry("y").or_insert_with(|| json!({}));
            if let Some(y) = y.as_object_mut() {
                y.insert("strip".into(), json!(false));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A value ruler is a field beside its view**, facing it, and the view
    /// beside it keeps no strip of its own -- whatever its vertical already
    /// said.
    #[test]
    fn a_value_ruler_faces_its_view_and_the_view_keeps_no_strip() {
        let ruler = ValueRuler {
            id: 30,
            rules: 18,
            side: Side::Right,
        };
        let strip = ruler.widget();
        assert_eq!(strip["type"], "field");
        assert_eq!(strip["axes"]["y"]["rules"], 18);
        assert_eq!(
            strip["axes"]["y"]["dir"], "left",
            "it faces the view on its left"
        );
        let left = ValueRuler {
            side: Side::Left,
            ..ruler
        };
        assert_eq!(left.widget()["axes"]["y"]["dir"], "right", "the mirror");
        let mut view = Map::new();
        view.insert("axes".into(), json!({"y": {"unit": "db"}}));
        ValueRuler::rule(&mut view);
        assert_eq!(view["axes"]["y"]["unit"], "db");
        assert_eq!(view["axes"]["y"]["strip"], false);
    }

    /// **The strip is a field over its group, in its unit**, and a view
    /// under it counts in the same unit and draws no strip of its own.
    #[test]
    fn the_strip_rules_its_group_and_the_views_under_it_draw_none() {
        let mut axis = Map::new();
        axis.insert("playhead_at".into(), json!(0.0));
        let ruler = TimeRuler {
            id: 7,
            unit: Unit::Samples,
            link: 40,
            rate: 48_000.0,
            side: Side::Top,
            axis,
        };
        let strip = ruler.widget();
        assert_eq!(strip["type"], "field");
        assert_eq!(strip["axes"]["x"]["unit"], "samples");
        assert_eq!(strip["axes"]["x"]["link"], 40);
        assert_eq!(strip["axes"]["x"]["playhead_at"], 0.0);
        assert_eq!(strip["axes"]["x"]["dir"], "down", "over the views");
        let under = TimeRuler {
            side: Side::Bottom,
            ..ruler.clone()
        };
        assert_eq!(under.widget()["axes"]["x"]["dir"], "up", "the mirror");
        let mut view = Map::new();
        view.insert("axes".into(), json!({"x": {"unit": "beats"}}));
        ruler.rule(&mut view);
        assert_eq!(view["axes"]["x"]["unit"], "samples");
        assert_eq!(view["axes"]["x"]["strip"], false);
        let mut flat = Map::new();
        flat.insert("ruler".into(), json!("beats"));
        ruler.rule(&mut flat);
        assert_eq!(flat["ruler"], "samples");
        assert_eq!(flat["ruler_strip"], false);
    }

    /// The verbs and the names round trip, and a name no unit has is refused.
    #[test]
    fn a_unit_is_chosen_by_its_verb_or_its_name() {
        for (verb, unit) in VERBS {
            assert_eq!(Unit::of_verb(verb), Some(*unit));
            assert_eq!(Unit::named(unit.prop()), Some(*unit));
            assert_eq!(unit.chosen_by(verb), Some(true));
        }
        assert!(Unit::parse("bars").is_err());
    }
}
