//! The `multitrack` widget: one element that owns a stack of lanes and the
//! clips on them.
//!
//! It is the `pianoroll`'s shape applied to a piece. A roll is one widget
//! holding its notes; this is one widget holding its lanes and clips, drawing
//! its own headers, its own stack and its own boxes on the shared time axis.
//! What that replaces is a *tree* of `Track` widgets under whatever generic
//! container a script picked — a shape with nobody in it that owned the piece,
//! so a gesture had nowhere to report *the piece* and reported what the hand
//! did to whichever widget it touched. `clients/gui/PLAN.md`'s `G34` carries
//! the whole argument.
//!
//! **The lanes and the clips are props**, flat like a roll's `notes`: a client
//! describes the piece and never composes a tree of it, never registers a
//! handler per box, and never learns a widget id. Identity is the client's own
//! name, so what comes back names what the script already knows.
//!
//! **It places; a clip is entered to edit.** A clip's contents draw read-only
//! here — this widget owns *where* things are, not what is inside them — and
//! editing one is opening it in the editor its structure asks for. That is what
//! keeps a heavy widget from becoming every widget, and it is the line the three
//! applications are drawn on: the multitrack places, the audio editor and the
//! score editor edit.

use serde_json::{Map, Value};

use crate::host::font;
use crate::host::graphics::multitrack::{self as model, Clip, Lane};
use crate::host::graphics::track;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::placement::Placement;
use crate::host::widget::element::{Ctx, Element, TimeSpace};
use crate::host::widget::parse::{self, label, number, number_f64, truthy};
use crate::host::widget::size::Natural;
use crate::host::widget::{EditorProps, RulerY};
use crate::viewport::View;

/// The gap between lanes, in logical pixels, when the props name none.
const GAP: f32 = 4.0;

/// A lane's thickness when nothing says otherwise.
const LANE_H: f32 = 96.0;

/// The narrowest a clip's box is drawn at, so one nobody can see never becomes
/// one nobody can grab.
const MIN_CLIP_W: f32 = 3.0;

/// The stack of lanes and the clips on them.
#[derive(Debug, Clone)]
pub struct Multitrack {
    /// The lanes, top to bottom.
    pub(crate) lanes: Vec<Lane>,
    /// The clips, each naming the lane it is on.
    pub(crate) clips: Vec<Clip>,
    /// Which clips the hand is holding, by index. **The hand's, not the
    /// piece's**: nothing on the wire sets or reports it, exactly as nothing
    /// reports which notes a roll has selected.
    pub(crate) selected: Vec<usize>,
    /// How far the stack is scrolled, in logical pixels. Screen state, so it
    /// survives a redefine rather than being restated by every def.
    pub(crate) scroll: f32,
    /// The space between lanes.
    pub(crate) gap: f32,
    /// The grid a placement lands on, in timeline samples; `0` is no grid.
    pub(crate) snap: f64,
    /// The shared axis, the selection and the playhead — the same props every
    /// timeline view carries.
    pub(crate) editor: EditorProps,
    /// A caption drawn in the corner.
    pub(crate) label: Option<String>,
}

impl Default for Multitrack {
    fn default() -> Self {
        Self {
            lanes: Vec::new(),
            clips: Vec::new(),
            selected: Vec::new(),
            scroll: 0.0,
            gap: GAP,
            snap: 0.0,
            editor: EditorProps::body(),
            label: None,
        }
    }
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `multitrack` node carries, read once — shared by the constructor
/// and by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Multitrack {
    Multitrack {
        lanes: parse_lanes(props),
        clips: parse_clips(props),
        selected: Vec::new(),
        scroll: 0.0,
        gap: number(props, "gap", GAP).max(0.0),
        snap: number_f64(props, "snap", 0.0).max(0.0),
        editor: EditorProps::parse(props, RulerY::Off),
        label: label(props),
    }
}

/// The `lanes` prop: the flat `name label height mute solo gain` sextuple array.
///
/// A trailing partial group is dropped rather than half-read, which is the rule
/// every flat payload here follows.
fn parse_lanes(props: &Map<String, Value>) -> Vec<Lane> {
    let Some(Value::Array(items)) = props.get("lanes") else {
        return Vec::new();
    };
    items
        .as_chunks::<6>()
        .0
        .iter()
        .filter_map(|c| {
            Some(Lane {
                name: c[0].as_str()?.to_string(),
                label: c[1].as_str().unwrap_or_default().to_string(),
                height: c[2].as_f64().unwrap_or(f64::from(LANE_H)) as f32,
                mute: truthy(&c[3]).unwrap_or(false),
                solo: truthy(&c[4]).unwrap_or(false),
                gain: c[5].as_f64().unwrap_or(1.0) as f32,
            })
        })
        .collect()
}

/// The `clips` prop: the flat `name lane offset dur start label` sextuple array.
fn parse_clips(props: &Map<String, Value>) -> Vec<Clip> {
    let Some(Value::Array(items)) = props.get("clips") else {
        return Vec::new();
    };
    items
        .as_chunks::<6>()
        .0
        .iter()
        .filter_map(|c| {
            Some(Clip {
                name: c[0].as_str()?.to_string(),
                lane: c[1].as_str().unwrap_or_default().to_string(),
                place: Placement {
                    offset: c[2].as_f64().unwrap_or(0.0).max(0.0),
                    dur: c[3].as_f64().unwrap_or(0.0).max(0.0),
                    start: c[4].as_f64().unwrap_or(0.0),
                },
                label: c[5].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

impl Multitrack {
    /// The axis this draws against: the navigation group's window when it is on
    /// one, else its own whole extent.
    fn view(&self, time: Option<TimeSpace>) -> View {
        match time {
            Some(t) => t.view,
            None => View::full(model::extent(&self.clips).ceil().max(1.0) as usize),
        }
    }

    /// The lane a clip sits on, by index — `None` for a clip naming a lane that
    /// is not here.
    ///
    /// **A clip is kept rather than dropped**, because what cannot be placed can
    /// still be reported: a script that renamed a lane gets its clips back to
    /// re-home rather than silently losing them.
    fn lane_of(&self, clip: &Clip) -> Option<usize> {
        self.lanes.iter().position(|l| l.name == clip.lane)
    }

    /// The lane header a lane's own props ask for. Presence-driven, like every
    /// header here: a lane that carries no mixer state offers no controls.
    fn header(&self, lane: &Lane, indent: f32) -> track::Header {
        track::Header {
            w: (indent > 0.0).then_some(indent),
            mute: Some(lane.mute),
            solo: Some(lane.solo),
            level: Some(lane.gain),
        }
    }
}

impl Element for Multitrack {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // A non-scalar rides a `/gui_set` as its JSON string, the carrier
            // every structure on this wire uses.
            "lanes" => {
                self.lanes = parse_lanes(&parse::as_array_props("lanes", v));
                true
            }
            "clips" => {
                self.clips = parse_clips(&parse::as_array_props("clips", v));
                self.selected.clear();
                true
            }
            "gap" => {
                self.gap = v.as_f64().unwrap_or(f64::from(GAP)).max(0.0) as f32;
                true
            }
            "snap" => {
                self.snap = v.as_f64().unwrap_or(0.0).max(0.0);
                true
            }
            "label" => {
                self.label = v.as_str().map(str::to_string);
                true
            }
            _ => self.editor.apply(key, v),
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let nav = self.view(ctx.time);
        let at = model::stack(&self.lanes, ctx.rect, self.scroll, self.gap);
        // A lane scrolled off either end is skipped rather than drawn and
        // clipped: `stack` reports every lane so a hit test reads the same
        // rects, and the drawing is what decides it has nothing to do.
        let shown = |r: Rect| r.y + r.h >= ctx.rect.y && r.y <= ctx.rect.y + ctx.rect.h;
        for (i, lane) in self.lanes.iter().enumerate() {
            if shown(at[i]) {
                track::draw(
                    d,
                    at[i],
                    Some(lane.shown()),
                    &self.header(lane, ctx.indent),
                    false,
                    ctx.indent,
                );
            }
        }
        // **One pass over the clips, each onto the lane it names.** A clip
        // whose lane is gone draws nowhere and is still held, which is what
        // lets it come back in a report to be re-homed.
        for (n, clip) in self.clips.iter().enumerate() {
            let Some(i) = self.lane_of(clip) else {
                continue;
            };
            if !shown(at[i]) {
                continue;
            }
            let body = track::lane_body(at[i], false, ctx.indent, ctx.metrics);
            if body.w <= 0.0 || body.h <= 0.0 {
                continue;
            }
            let Some((x0, x1)) = model::clip_x(clip, body, &nav, MIN_CLIP_W) else {
                continue;
            };
            let cr = track::clip_rect(body, x0, x1);
            track::draw_clip(d, cr, self.selected.contains(&n));
            track::draw_clip_label(d, cr, clip.shown());
        }
        if let Some(text) = &self.label {
            let (mesh, m, theme) = d.parts();
            font::text(
                mesh,
                text,
                ctx.rect.x + ctx.indent + m.pad,
                ctx.rect.y + 2.0,
                m.caption_scale,
                theme.ruler_text,
            );
        }
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }

    /// **Elastic on both axes.** It is a surface whose extent is the caller's,
    /// and its own content must never size it: a lane added by a `/gui_set`
    /// would then relayout the window, which is both a visible jump and a cost
    /// per message.
    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        Natural::default()
    }

    fn editor(&self) -> Option<&EditorProps> {
        Some(&self.editor)
    }

    fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        Some(&mut self.editor)
    }

    fn navigates_time(&self) -> bool {
        true
    }

    /// How far the piece reaches on the axis — what an autofit and a scroll
    /// size themselves against.
    fn content_span(&self) -> Option<f64> {
        Some(model::extent(&self.clips))
    }

    /// What `/gui_query` overlays: the two structures, each as the string its
    /// own `/gui_set` would take.
    fn info(&self) -> Vec<(String, Value)> {
        vec![
            ("lanes".into(), model::lanes_json(&self.lanes)),
            ("clips".into(), model::clips_json(&self.clips)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(json: &str) -> Map<String, Value> {
        match serde_json::from_str(json).expect("valid JSON") {
            Value::Object(m) => m,
            _ => panic!("an object"),
        }
    }

    const TWO: &str = r#"{
        "lanes": ["noise", "", 100, 0, 0, 0.8, "tone", "Lead", 60, 1, 0, 0.5],
        "clips": ["a", "noise", 0, 48000, 0, "", "b", "tone", 96000, 48000, 0, "take 2"]
    }"#;

    /// **A client describes the piece; it does not compose a tree of it.** The
    /// two structures arrive as flat arrays, like a roll's notes, and the widget
    /// is the one thing that holds them.
    #[test]
    fn the_lanes_and_the_clips_are_props_of_one_widget() {
        let mt = from_props(&props(TWO));
        assert_eq!(mt.lanes.len(), 2);
        assert_eq!(mt.clips.len(), 2);

        assert_eq!(mt.lanes[1].name, "tone");
        assert_eq!(
            mt.lanes[1].shown(),
            "Lead",
            "the label draws, the name addresses"
        );
        assert!(mt.lanes[1].mute);
        assert_eq!(mt.lanes[1].gain, 0.5);

        assert_eq!(mt.clips[1].lane, "tone");
        assert_eq!(mt.clips[1].place.offset, 96_000.0);
        assert_eq!(mt.clips[1].shown(), "take 2");
    }

    /// A trailing partial group is dropped rather than half-read, and a lane
    /// with no name is not a lane — the one field that is the identity.
    #[test]
    fn a_partial_group_is_dropped_rather_than_half_read() {
        let mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1, "two", ""],
                "clips": ["a", "one", 0, 10, 0]}"#,
        ));
        assert_eq!(mt.lanes.len(), 1, "the second group is short");
        assert!(mt.clips.is_empty(), "so is the only clip");

        let unnamed = from_props(&props(r#"{"lanes": [7, "", 100, 0, 0, 1]}"#));
        assert!(unnamed.lanes.is_empty(), "a lane with no name is not one");
    }

    /// **A `/gui_set` of a structure is the same parse**, so what a query
    /// reports is what a set would take — the round trip every non-scalar here
    /// keeps.
    #[test]
    fn a_set_reads_what_a_query_reported() {
        let mut mt = from_props(&props(TWO));
        let reported: Map<String, Value> = mt.info().into_iter().collect();

        let mut empty = Multitrack::default();
        assert!(empty.set("lanes", &reported["lanes"]));
        assert!(empty.set("clips", &reported["clips"]));
        assert_eq!(empty.lanes, mt.lanes);
        assert_eq!(empty.clips, mt.clips);

        // And the same through the string carrier a `/gui_set` actually uses.
        let as_text = Value::from(serde_json::to_string(&reported["clips"]).unwrap());
        mt.clips.clear();
        assert!(mt.set("clips", &as_text));
        assert_eq!(mt.clips.len(), 2);
    }

    /// A clip naming a lane that is not here is **kept**, not dropped: what
    /// cannot be placed can still be reported, so a script that renamed a lane
    /// gets its clips back to re-home instead of losing them.
    #[test]
    fn a_clip_on_a_lane_that_is_gone_is_kept_and_not_placed() {
        let mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1],
                "clips": ["a", "one", 0, 10, 0, "", "b", "vanished", 0, 10, 0, ""]}"#,
        ));
        assert_eq!(mt.clips.len(), 2);
        assert!(mt.lane_of(&mt.clips[0]).is_some());
        assert!(mt.lane_of(&mt.clips[1]).is_none());
        // It is still in what the widget reports, which is how it comes back.
        let Value::Array(written) = model::clips_json(&mt.clips) else {
            panic!("an array");
        };
        assert_eq!(written.len(), 12);
    }

    /// **Its size is the caller's, never its content's.** A lane added by a
    /// `/gui_set` must not relayout the window.
    #[test]
    fn a_lane_added_never_changes_how_big_the_widget_wants_to_be() {
        let m = Metrics::default();
        let mut mt = Multitrack::default();
        let bare = mt.natural(&m, 1.0);
        mt.set("lanes", &props(TWO)["lanes"]);
        assert_eq!(mt.natural(&m, 1.0), bare);
        // What *does* follow the content is how far the axis reaches.
        assert_eq!(mt.content_span(), Some(0.0));
        mt.set("clips", &props(TWO)["clips"]);
        assert_eq!(mt.content_span(), Some(144_000.0));
    }
}
