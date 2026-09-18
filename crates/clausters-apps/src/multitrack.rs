//! **The multitrack editor**: the window a multitrack opens in.
//!
//! What the editor shows is three things composed in one window, and only one
//! of them is the multitrack:
//!
//! - a **time ruler** above it, on the multitrack's own axis, which is where the
//!   position cursor is placed and nowhere else;
//! - the **multitrack** widget, which draws the rows and the boxes and draws no
//!   ruler of its own;
//! - when the multitrack can be heard, the **transport row**: rewind, play/pause,
//!   stop, and a clock reading where the multitrack is.
//!
//! That arrangement is the application's and not the widget's. A widget draws
//! one structure; an editor is what puts a structure beside the controls that
//! act on it, and a window composed differently in two places is two editors.
//!
//! # The ids are the caller's
//!
//! A widget id is a running host's fact, so the window is composed around the
//! ids it is handed. The ruler and the multitrack are always numbered, because what a
//! hand does on them has to come back to the editor that drew them. The
//! transport row's widgets are addressed by **name** ([`REWIND`], [`PLAY`],
//! [`STOP`], [`CLOCK`]), so a caller that numbers id-less widgets on the way out
//! — both clients do — leaves them unnumbered ([`Transport::Unnumbered`]), and
//! a host composing a window for itself numbers them here
//! ([`Transport::Numbered`]).

use serde::Deserialize;
use serde_json::{Map, Value, json};

use clausters_core::tempoclock::secs_to_samples;
use clausters_core::tempomap::TempoMap;
use clausters_document::multitrack::Multitrack;
use clausters_editing::multitrack::{self as projection, Look};

pub mod editor;

/// The name of the transport row's rewind button.
pub const REWIND: &str = "multitrack_rewind";
/// The name of the transport row's play/pause button.
pub const PLAY: &str = "multitrack_play";
/// The name of the transport row's stop button.
pub const STOP: &str = "multitrack_stop";
/// The name of the label that reads where the multitrack is.
pub const CLOCK: &str = "multitrack_clock";

/// The strip that rules the multitrack, in logical pixels.
const RULER_H: f64 = 20.0;

/// The ids of the transport row, for a caller that numbers them itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct TransportIds {
    /// The row that holds the other four. A client that learns the other four
    /// by name after opening the window has no name for this one, and nothing
    /// is addressed to it.
    #[serde(default)]
    pub row: i32,
    /// [`REWIND`].
    pub rewind: i32,
    /// [`PLAY`].
    pub play: i32,
    /// [`STOP`].
    pub stop: i32,
    /// [`CLOCK`].
    pub clock: i32,
}

/// Whether the window carries the transport row, and who numbers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// A multitrack nobody can play: it still edits, and it has nothing to play
    /// with.
    Absent,
    /// The row, with its widgets named and unnumbered — for a caller that
    /// numbers id-less widgets when it sends the window.
    Unnumbered,
    /// The row, numbered here.
    Numbered(TransportIds),
}

/// Where one track's meters are read from: the control buses its level and its
/// held mark are written to, `channels` of each, the level first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct Meter {
    /// The track, by its id in the multitrack.
    pub track: u64,
    /// The first bus of the level run.
    pub bus: i32,
    /// How many channels each run is.
    pub channels: usize,
}

/// Everything a window over a multitrack is composed from.
pub struct Window<'a> {
    /// The multitrack.
    pub multitrack: &'a Multitrack,
    /// The rate a second lands at and which buffer each source was read into.
    pub look: &'a Look<'a>,
    /// The tempo map the multitrack holds, which is what the ruler draws its beats
    /// and bars from. It places nothing: the multitrack is in seconds.
    pub tempo: &'a TempoMap,
    /// The id of the multitrack's own widget.
    pub widget: i32,
    /// The id of the strip that rules it.
    pub ruler: i32,
    /// The navigation group the multitrack and its ruler share, when the caller names
    /// one.
    pub link: Option<i64>,
    /// The position cursor, in seconds — `None` until a hand places one.
    pub cursor: Option<f64>,
    /// Where each track's meters are read from; empty for a multitrack nobody plays.
    pub meters: &'a [Meter],
    /// The transport row.
    pub transport: Transport,
    /// The window's title.
    pub title: &'a str,
    /// The window's size, in logical pixels.
    pub size: (i64, i64),
}

impl Window<'_> {
    /// **The navigation group the multitrack and its ruler share.**
    ///
    /// A ruler rules by being on the same axis as what it is beside, and an
    /// unlinked widget is a group of one keyed by itself — so the two would pan
    /// and zoom apart. The multitrack's own widget id names the group when the caller
    /// did not name one, which is the id nothing else can collide with.
    pub fn group(&self) -> i64 {
        self.link.unwrap_or(i64::from(self.widget))
    }

    /// The position cursor in timeline samples: where it was placed, and the
    /// top of the multitrack until a hand places one. A multitrack that stated no cursor
    /// would otherwise open with nowhere to play from.
    pub fn cursor_units(&self) -> f64 {
        secs_to_samples(self.cursor.unwrap_or(0.0), self.look.rate) as f64
    }

    /// The tempo map as the wire carries it: the JSON breakpoint list, as a
    /// string, because OSC carries no arrays and a `/gui_set` of it could not be
    /// spelled otherwise.
    fn tempo_map(&self) -> String {
        serde_json::to_string(self.tempo).unwrap_or_default()
    }
}

/// **The window**, as a GuiDef rooted at a `window` node.
///
/// The ruler first, the multitrack under it and the transport row under that. The
/// root carries no id: a GuiDef's id is the one its `/gui_def` names.
///
/// **A script's own widgets are not composed here.** A client may append some
/// after the picture, and they are its objects — a widget built over a live
/// source keeps a binding no JSON carries — so it appends them to the children
/// this answers.
pub fn window(w: &Window<'_>) -> Value {
    let mut multitrack = Map::new();
    multitrack.insert("type".into(), json!("multitrack"));
    multitrack.insert("id".into(), json!(w.widget));
    multitrack.extend(multitrack_props(w));
    let mut children = vec![ruler(w), Value::Object(multitrack)];
    match w.transport {
        Transport::Absent => {}
        Transport::Unnumbered => children.push(transport(None)),
        Transport::Numbered(ids) => children.push(transport(Some(ids))),
    }
    json!({
        "type": "window",
        "title": w.title,
        "w": w.size.0,
        "h": w.size.1,
        "flow": "col",
        "children": children,
    })
}

/// **Everything a widget of this window should be drawing**, for a correction.
///
/// Not only what a gesture touched: a correction is the answer to an edit that
/// arrived too late or was refused, which is the one case where the host's
/// whole picture of a widget is in doubt. The ruler's own state is its cursor
/// and nothing else; every other id is answered as the multitrack.
pub fn props(w: &Window<'_>, widget: i32) -> Map<String, Value> {
    if widget == w.ruler {
        let mut out = Map::new();
        out.insert("cursor".into(), json!(w.cursor_units()));
        return out;
    }
    multitrack_props(w)
}

/// The multitrack widget's props: the projection's, and what the window adds to it.
fn multitrack_props(w: &Window<'_>) -> Map<String, Value> {
    // **The multitrack's own props are the projection's**: the rows, the boxes, the
    // automations over both, their break-points, which are hidden and which
    // boxes loop. What is added here is a function of something other than the
    // multitrack.
    let mut props = projection::props(w.multitrack, w.look);
    // **Where each track's level is read from**: the control buses its meters
    // write, read by the host every frame straight out of the shared segment.
    let meters: Vec<Value> = w
        .meters
        .iter()
        .flat_map(|m| {
            [
                json!(m.track.to_string()),
                json!(m.bus),
                json!(m.bus + m.channels as i32),
                json!(m.channels),
            ]
        })
        .collect();
    props.insert("meters".into(), Value::Array(meters));
    props.insert("weight".into(), json!(1.0));
    props.insert("ruler".into(), json!("beats"));
    props.insert("sample_rate".into(), json!(w.look.rate));
    // **The window is the reader's.** In an editor a content change is mostly
    // the reader's own edit, so the axis does not re-frame itself on one.
    props.insert("autofit".into(), json!(false));
    // The head is anchored at 0 because the counter it sweeps from is already
    // the multitrack's position.
    props.insert("playhead_at".into(), json!(0.0));
    // The ruler's beats and bars are the multitrack's own tempo map drawn over its
    // seconds: the ruler's configuration, which moves no box.
    props.insert("tempo_map".into(), json!(w.tempo_map()));
    props.insert("cursor".into(), json!(w.cursor_units()));
    props.insert("link".into(), json!(w.group()));
    props
}

/// The free-standing strip that rules the multitrack from above: its marks hug its
/// bottom edge, which is a ruler's default there.
fn ruler(w: &Window<'_>) -> Value {
    json!({
        "type": "field",
        "id": w.ruler,
        "h": RULER_H,
        "axes": {"x": {
            "unit": "beats",
            "tempo_map": w.tempo_map(),
            "sample_rate": w.look.rate,
            "link": w.group(),
            // **The same anchor as the multitrack's**, because the ruler comes
            // first: a linked group is seeded by its first member, so a ruler
            // with no anchor left the whole window's head parked and only a
            // client's later `/gui_set` ever swept it.
            "playhead_at": 0.0,
            "cursor": w.cursor_units(),
        }},
    })
}

/// **The transport row**: rewind, play/pause, stop, and where the multitrack is.
///
/// **Rewind is not stop.** Stop goes back to the *mark* — which is what tells
/// it from pause — and the mark is wherever a hand last put it, so with nothing
/// else the way back to the top is finding beat zero on screen and clicking it.
/// Rewind puts the mark there, which is a statement about the cursor and not
/// about the transport.
fn transport(ids: Option<TransportIds>) -> Value {
    let numbered = |mut node: Value, id: Option<i32>| {
        if let (Some(id), Some(map)) = (id, node.as_object_mut()) {
            map.insert("id".into(), json!(id));
        }
        node
    };
    let button = |label: &str, name: &str, width: f64, id: Option<i32>| {
        numbered(
            json!({"type": "button", "label": label, "name": name, "w": width}),
            id,
        )
    };
    let children = vec![
        button("|<", REWIND, 44.0, ids.map(|i| i.rewind)),
        button("play/pause", PLAY, 110.0, ids.map(|i| i.play)),
        button("stop", STOP, 110.0, ids.map(|i| i.stop)),
        numbered(
            json!({"type": "label", "text": "", "name": CLOCK, "text_size": 2.0, "weight": 1.0}),
            ids.map(|i| i.clock),
        ),
    ];
    numbered(
        json!({"type": "layout", "flow": "row", "h": 40.0, "gap": 6.0, "children": children}),
        ids.map(|i| i.row),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::multitrack::{Content, Region, Tempo, Track};
    use clausters_document::{Beat, NodeId, Second, SourceId};
    use std::collections::HashMap;

    /// One track holding one box, at a tempo of two beats a second.
    fn multitrack() -> Multitrack {
        let region = Region::new(
            NodeId(3),
            Second(4.0),
            Second(4.0),
            Content::Unknown(Value::Null),
        );
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.lanes[0].regions.push(region);
        let mut multitrack = Multitrack::default();
        multitrack.tracks.push(track);
        multitrack.tempo.push(Tempo::at(Beat(0.0), 2.0));
        multitrack
    }

    fn compose<T>(
        transport: Transport,
        cursor: Option<f64>,
        f: impl FnOnce(&Window<'_>) -> T,
    ) -> T {
        let multitrack = multitrack();
        let tempo = projection::tempo_map(&multitrack);
        let table: HashMap<SourceId, i64> = HashMap::new();
        let look = Look {
            rate: 48_000.0,
            sources: &table,
        };
        let meters = [Meter {
            track: 1,
            bus: 20,
            channels: 2,
        }];
        f(&Window {
            multitrack: &multitrack,
            look: &look,
            tempo: &tempo,
            widget: 7,
            ruler: 8,
            link: None,
            cursor,
            meters: &meters,
            transport,
            title: "multitrack",
            size: (1000, 560),
        })
    }

    /// **A ruler above the multitrack, on its axis, and the transport under it.**
    #[test]
    fn the_window_is_a_ruler_above_the_multitrack_and_the_transport_below() {
        let def = compose(Transport::Unnumbered, None, window);
        assert_eq!(def["type"], "window");
        assert_eq!(def["flow"], "col");
        let children = def["children"].as_array().unwrap();
        assert_eq!(children.len(), 3);
        let (ruler, multitrack, row) = (&children[0], &children[1], &children[2]);
        assert_eq!(ruler["type"], "field");
        assert_eq!(ruler["id"], 8);
        assert_eq!(multitrack["type"], "multitrack");
        assert_eq!(multitrack["id"], 7);
        assert_eq!(
            ruler["axes"]["x"]["link"], multitrack["link"],
            "one axis, not two"
        );
        assert_eq!(ruler["axes"]["x"]["unit"], "beats");
        assert_eq!(
            ruler["axes"]["x"]["playhead_at"], multitrack["playhead_at"],
            "the first member seeds the group's anchor, so the ruler states it too"
        );
        let names: Vec<&str> = row["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, [REWIND, PLAY, STOP, CLOCK]);
        assert!(
            row.get("id").is_none() && row["children"][0].get("id").is_none(),
            "named, and numbered by whoever sends it"
        );
    }

    /// A multitrack nobody can play has no transport row.
    #[test]
    fn a_multitrack_nobody_plays_has_no_transport() {
        let def = compose(Transport::Absent, None, window);
        assert_eq!(def["children"].as_array().unwrap().len(), 2);
    }

    /// **A host composing a window for itself numbers every widget**, because
    /// a child with no id is a child its registry skips.
    #[test]
    fn a_window_numbered_here_leaves_no_widget_without_an_id() {
        let ids = TransportIds {
            row: 9,
            rewind: 10,
            play: 11,
            stop: 12,
            clock: 13,
        };
        let def = compose(Transport::Numbered(ids), None, window);
        fn unnumbered(node: &Value) -> usize {
            let own = usize::from(node.get("id").is_none());
            own + node["children"]
                .as_array()
                .map_or(0, |c| c.iter().map(unnumbered).sum())
        }
        assert_eq!(unnumbered(&def), 1, "only the root, whose id is the def's");
    }

    /// **The multitrack's props are the projection's and the window's**: the rows
    /// and boxes, the meters as the widget's quadruples, and a cursor in
    /// seconds that crosses to samples by the rate alone, whatever the tempo.
    #[test]
    fn the_multitrack_is_drawn_from_the_projection_and_the_window() {
        let props = compose(Transport::Absent, Some(2.0), |w| props(w, w.widget));
        assert!(props.contains_key("lanes") && props.contains_key("clips"));
        assert_eq!(props["meters"], json!(["1", 20, 22, 2]));
        assert_eq!(
            props["cursor"],
            json!(96_000.0),
            "two seconds, at two beats a second or at any other tempo"
        );
        assert_eq!(
            props["link"],
            json!(7),
            "the multitrack's own id names the group"
        );
        let ruler = compose(Transport::Absent, None, |w| super::props(w, w.ruler));
        assert_eq!(
            Value::Object(ruler),
            json!({"cursor": 0.0}),
            "the ruler's state is its cursor, and the top until one is placed"
        );
    }
}
