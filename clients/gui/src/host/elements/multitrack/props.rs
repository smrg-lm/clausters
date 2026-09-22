//! **What a script says the multitrack is**: the props in, and the reports out.
//!
//! One direction is the wire's lists becoming this widget's own -- the lanes,
//! the boxes, the curves, the layers, the break-points and the names that tell
//! a minted word from the multitrack's own id -- and the other is every payload a
//! gesture leaves. They are one module because they are one contract read from
//! either end: a report is written in exactly the shape the prop it answers
//! would take, so applying what came back is the identity.

use super::*;

/// One JSON scalar as the OSC primitive it is -- **flat primitives at the
/// boundary**, which is what every edit-back payload here rides as.
pub(super) fn json_arg(v: Value) -> OscType {
    match v {
        Value::String(s) => OscType::String(s),
        Value::Number(n) if n.is_i64() => OscType::Int(n.as_i64().unwrap_or(0) as i32),
        // **A double, not a float**: these are positions in samples, and an
        // `f32` holds a whole sample only up to 2^24 of them -- under six
        // minutes at 48 kHz -- so a box past that came back somewhere else.
        other => OscType::Double(other.as_f64().unwrap_or(0.0)),
    }
}

/// The **body element** a box of samples is drawn through: the signal element
/// this build already has, named onto one server buffer and given the
/// presentation the widget asked for.
///
/// It is built through the ordinary constructor rather than by naming fields,
/// so a box's picture and a standalone `signal` are the same product of the
/// same props -- which is what keeps them from drifting when either grows a
/// prop. A body carries no chrome: the ruler, the gutter and the navigation
/// belong to the view that placed it.
pub(super) fn take_body(bufnum: i32, view: Presentation) -> SignalElement {
    let mut props = Map::new();
    props.insert("buffer".into(), Value::from(bufnum));
    props.insert("view".into(), Value::from(view.name()));
    crate::host::widget::signal_element(&props, &[]).unwrap_or_else(|_| {
        // The props above are this function's own and cannot be refused; the
        // fallback exists so a constructor that grows a rule does not take the
        // whole picture down with it.
        crate::host::widget::signal_element(&Map::new(), &[]).expect("a bare signal element")
    })
}

pub(crate) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `multitrack` node carries, read once -- shared by the constructor
/// and by the tests beside it.
pub(super) fn from_props(props: &Map<String, Value>) -> Multitrack {
    let curves = parse_curves(props);
    let layers = parse_layers(props);
    Multitrack {
        lanes: parse_lanes(props),
        clips: parse_clips(props),
        track: None,
        bodies: curve_bodies(&curves, &layers, &parse_points(props)),
        curves,
        layers,
        layer: props.get("layer").and_then(Value::as_str).and_then(named),
        hidden: parse_hidden(props),
        loops: parse_names(props, "loops"),
        rates: parse_rates(props),
        segments: parse_segments(props),
        meters: parse_meters(props),
        zoom: HashMap::new(),
        curve_zoom: HashMap::new(),
        holding: None,
        selected: Vec::new(),
        scroll: 0.0,
        gap: number(props, "gap", GAP).max(0.0),
        snap: number_f64(props, "snap", 0.0).max(0.0),
        editor: EditorProps::parse(props, RulerY::Off),
        sizing: None,
        label: label(props),
        view: props
            .get("view")
            .and_then(Value::as_str)
            .and_then(Presentation::parse)
            .unwrap_or(Presentation::Signal),
        takes: HashMap::new(),
        asked: std::collections::HashSet::new(),
        rolls: parse_notes(props)
            .into_iter()
            .map(|(name, notes)| (name, roll_body(&notes)))
            .collect(),
        pending: HashMap::new(),
        grab: None,
        block: Vec::new(),
        fading: None,
    }
}

/// The `lanes` prop: the flat `name label height mute solo gain` sextuple array.
///
/// A trailing partial group is dropped rather than half-read, which is the rule
/// every flat payload here follows.
pub(super) fn parse_lanes(props: &Map<String, Value>) -> Vec<Lane> {
    let Some(Value::Array(items)) = props.get("lanes") else {
        return Vec::new();
    };
    items
        .as_chunks::<7>()
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
                curves: truthy(&c[6]).unwrap_or(true),
            })
        })
        .collect()
}

/// **What one track's meter is read out of**: two runs of control buses, one
/// value per channel each.
///
/// Two buses and not one because a meter shows two things -- the level, and the
/// mark that waits to be read -- and they are the same measurement with
/// different ballistics, which is why the server writes both rather than
/// letting two clients invent two falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaneMeter {
    /// The first control bus of the level run.
    pub(super) level: i32,
    /// The first control bus of the held-peak run; negative for no mark.
    pub(super) mark: i32,
    /// How many channels the track has, which is how long each run is.
    pub(super) channels: usize,
}

/// The `meters` prop: the flat `lane level mark channels` quadruple array.
///
/// Its own prop rather than two more fields on `lanes`, because a meter is not
/// a thing this widget reports back: the `lanes` payload is an **edit** a hand
/// made, and a bus number in it would be a bus number the host was expected to
/// return unchanged.
pub(super) fn parse_meters(props: &Map<String, Value>) -> HashMap<String, LaneMeter> {
    let Some(Value::Array(items)) = props.get("meters") else {
        return HashMap::new();
    };
    items
        .as_chunks::<4>()
        .0
        .iter()
        .filter_map(|c| {
            let channels = c[3].as_i64().unwrap_or(0).max(0) as usize;
            if channels == 0 {
                return None;
            }
            Some((
                c[0].as_str()?.to_string(),
                LaneMeter {
                    level: c[1].as_i64().unwrap_or(-1) as i32,
                    mark: c[2].as_i64().unwrap_or(-1) as i32,
                    channels,
                },
            ))
        })
        .collect()
}

/// The `clips` prop: the flat `name lane offset dur start label source`
/// septuple array.
pub(super) fn parse_clips(props: &Map<String, Value>) -> Vec<Clip> {
    let Some(Value::Array(items)) = props.get("clips") else {
        return Vec::new();
    };
    items
        .as_chunks::<7>()
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
                source: c[6].as_i64().unwrap_or(i64::from(model::NO_SOURCE)) as i32,
            })
        })
        .collect()
}

/// The `curves` prop: the flat `name lane label min max height` sextuple array
/// -- a **track automation**, a row of its own under the lane it names.
pub(super) fn parse_curves(props: &Map<String, Value>) -> Vec<model::Curve> {
    let Some(Value::Array(items)) = props.get("curves") else {
        return Vec::new();
    };
    items
        .as_chunks::<6>()
        .0
        .iter()
        .filter_map(|c| {
            Some(model::Curve {
                name: c[0].as_str()?.to_string(),
                owner: c[1].as_str().unwrap_or_default().to_string(),
                label: c[2].as_str().unwrap_or_default().to_string(),
                min: c[3].as_f64().unwrap_or(0.0) as f32,
                max: c[4].as_f64().unwrap_or(1.0) as f32,
                height: c[5].as_f64().unwrap_or(f64::from(CURVE_H)) as f32,
            })
        })
        .collect()
}

/// The `layers` prop: the flat `name box label min max` quintuple array -- a
/// **clip envelope**, drawn inside the box it names.
///
/// It carries no height, and that is the shape saying what it is: a layer is as
/// tall as the box it is on, and a row is as tall as it asks.
pub(super) fn parse_layers(props: &Map<String, Value>) -> Vec<model::Curve> {
    let Some(Value::Array(items)) = props.get("layers") else {
        return Vec::new();
    };
    items
        .as_chunks::<5>()
        .0
        .iter()
        .filter_map(|c| {
            Some(model::Curve {
                name: c[0].as_str()?.to_string(),
                owner: c[1].as_str().unwrap_or_default().to_string(),
                label: c[2].as_str().unwrap_or_default().to_string(),
                min: c[3].as_f64().unwrap_or(0.0) as f32,
                max: c[4].as_f64().unwrap_or(1.0) as f32,
                height: CURVE_H,
            })
        })
        .collect()
}

/// The `points` prop: the flat `curve time value shape amount` quintuples,
/// gathered into the `time value shape amount` quads a curve reads.
///
/// **One list for every curve there is**, rows and layers alike, because a
/// break-point is a break-point wherever the curve hangs -- the same carrier a
/// roll's `notes` rides, with the curve's name in front the way a note names
/// its box. A point naming a curve that is not there is dropped.
pub(super) fn parse_points(props: &Map<String, Value>) -> HashMap<String, Vec<f64>> {
    let Some(Value::Array(items)) = props.get("points") else {
        return HashMap::new();
    };
    let mut out: HashMap<String, Vec<f64>> = HashMap::new();
    for q in items.as_chunks::<5>().0 {
        let Some(name) = q[0].as_str() else {
            continue;
        };
        out.entry(name.to_string())
            .or_default()
            .extend(q[1..].iter().map(|v| v.as_f64().unwrap_or(0.0)));
    }
    out
}

/// The `hidden` prop: the layers that are not drawn, space-separated.
pub(super) fn parse_hidden(props: &Map<String, Value>) -> Vec<String> {
    parse_names(props, "hidden")
}

/// A **name set** prop: the space-separated names under `key`, which is how
/// this widget spells a flag that belongs to some of what it holds -- the layers
/// that are not drawn, the boxes that wrap.
pub(super) fn parse_names(props: &Map<String, Value>, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The `segments` prop: the flat `box source start frames` quadruple array --
/// the spans each join is made of, in the order they play, grouped by the box
/// they belong to.
///
/// A trailing partial group is dropped rather than half-read, and so is a span
/// that reads nothing; a box whose spans are all dropped is not named, so it is
/// drawn from its own buffer as any box is.
pub(super) fn parse_segments(props: &Map<String, Value>) -> HashMap<String, Vec<super::Span>> {
    let Some(Value::Array(items)) = props.get("segments") else {
        return HashMap::new();
    };
    let mut out: HashMap<String, Vec<super::Span>> = HashMap::new();
    for group in items.as_chunks::<4>().0 {
        let (Some(name), Some(source)) = (group[0].as_str(), group[1].as_i64()) else {
            continue;
        };
        let frames = group[3].as_f64().unwrap_or(0.0);
        if source < 0 || frames <= 0.0 {
            continue;
        }
        out.entry(name.to_string()).or_default().push(super::Span {
            source: source as i32,
            start: group[2].as_f64().unwrap_or(0.0).max(0.0),
            frames,
        });
    }
    out
}

/// The `rates` prop: the flat `box rate` pairs -- how many frames of its source
/// one sample of that box is.
///
/// A box not named reads one frame per sample, which is every box over a source
/// written at the rate the multitrack is measured in. A rate that is zero or
/// negative is dropped rather than read: it would draw the samples backwards or
/// not at all, and a box whose rate nobody stated is the ordinary box.
pub(super) fn parse_rates(props: &Map<String, Value>) -> HashMap<String, f64> {
    let Some(Value::Array(items)) = props.get("rates") else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for pair in items.as_chunks::<2>().0 {
        let (Some(name), Some(rate)) = (pair[0].as_str(), pair[1].as_f64()) else {
            continue;
        };
        if rate > 0.0 {
            out.insert(name.to_string(), rate);
        }
    }
    out
}

/// A name set as a `/gui_set` value: the same space-separated list the prop
/// takes.
pub(super) fn names_of(v: &Value) -> Vec<String> {
    v.as_str()
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A layer name, or `None` for the two spellings that mean the placement -- the
/// boxes themselves, which is what a multitrack owns when no curve is in hand.
pub(super) fn named(v: &str) -> Option<String> {
    (!v.is_empty() && v != "placement").then(|| v.to_string())
}

/// The **body element** every curve is drawn and edited through: the `curve`
/// element this build already has, over that curve's own points and value
/// domain, with no chrome of its own.
///
/// Built once per curve rather than per placement, because the element *is* the
/// curve: a row and a layer differ in the rectangle and the span they are
/// handed at draw time, and in nothing they hold.
pub(super) fn curve_bodies(
    rows: &[model::Curve],
    layers: &[model::Curve],
    points: &HashMap<String, Vec<f64>>,
) -> HashMap<String, crate::host::elements::curve::Curve> {
    rows.iter()
        .chain(layers)
        .map(|c| {
            let mut props = Map::new();
            props.insert("min".into(), Value::from(c.min));
            props.insert("max".into(), Value::from(c.max));
            if let Some(flat) = points.get(&c.name) {
                props.insert("points".into(), Value::from(flat.clone()));
            }
            (c.name.clone(), crate::host::elements::curve::body(&props))
        })
        .collect()
}

/// The notes each box carries, by box name -- the flat
/// `box start dur pitch velocity channel` sextuples the wire takes, gathered
/// into the `start dur pitch velocity channel` quintuples a roll reads.
///
/// One list for the widget rather than one per box, because that is the shape
/// every payload here has: a flat list whose first fields are the identity. A
/// note naming a box that is not there is dropped -- unlike a clip, which is
/// kept and drawn nowhere, because a clip is what a report is *about* and a
/// note is what one holds.
pub(super) fn parse_notes(props: &Map<String, Value>) -> HashMap<String, Vec<f64>> {
    let Some(Value::Array(items)) = props.get("notes") else {
        return HashMap::new();
    };
    let mut out: HashMap<String, Vec<f64>> = HashMap::new();
    for n in items.as_chunks::<6>().0 {
        let Some(box_name) = n[0].as_str() else {
            continue;
        };
        out.entry(box_name.to_string())
            .or_default()
            .extend(n[1..].iter().map(|v| v.as_f64().unwrap_or(0.0)));
    }
    out
}

/// The **body element** a box of notes is drawn through: the roll this build
/// already has, over the notes of that one box and with every lane it draws on
/// its own turned off.
///
/// The pitch window is the crate's rule
/// ([`pitch_window`](clausters_document::view::catalogue::pitch_window)), the
/// same one a standalone roll's picture is fitted with -- so a box and a window
/// over the same notes agree about how tall they are.
pub(super) fn roll_body(notes: &[f64]) -> Notes {
    let (min, max) = clausters_document::view::catalogue::pitch_window(notes);
    let mut props = Map::new();
    props.insert("notes".into(), Value::from(notes.to_vec()));
    props.insert("min".into(), Value::from(min));
    props.insert("max".into(), Value::from(max));
    // A body has no chrome: no velocity lane, no marker lane, no ruler.
    props.insert("velocity".into(), Value::from(false));
    props.insert("osc_lane".into(), Value::from(false));
    props.insert("ruler".into(), Value::from("off"));
    // Read-only here, which is the line the whole widget is drawn on: the
    // multitrack places, and a box is **entered** to edit what is in it.
    props.insert("editable".into(), Value::from(false));
    crate::host::elements::notes::from_props(&props)
}
impl Multitrack {
    pub(super) fn apply_prop(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // A non-scalar rides a `/gui_set` as its JSON string, the carrier
            // every structure on this wire uses.
            "lanes" => {
                self.lanes = parse_lanes(&parse::as_array_props("lanes", v));
                self.zoom_rows();
                true
            }
            "clips" => {
                // **A correction does not empty the hand.** The list is
                // replaced whole, so the indices the selection holds mean
                // nothing afterwards -- but the boxes are the same boxes. This
                // is how they are found again, and it takes two passes because
                // a name is not always enough.
                //
                // **The name, and then where it is.** A box the *host* made --
                // the tail of a split, a paste -- carries a word this widget
                // minted, and the correction that comes back carries the
                // document's id instead: that is the whole point of the
                // correction. So the half the hand was still holding vanishes
                // from a lookup by name, and `j` found one box where the hand
                // held two -- and a join that joins nothing says nothing, so
                // the key looked dead. It is the same box in the same place on
                // the same lane, because the multitrack placed it exactly where this
                // said, and that is what the second pass matches on.
                let held: Vec<(String, String, f64)> = self
                    .selected
                    .iter()
                    .filter_map(|i| self.clips.get(*i))
                    .map(|c| (c.name.clone(), c.lane.clone(), c.place.offset))
                    .collect();
                self.clips = parse_clips(&parse::as_array_props("clips", v));
                let mut taken: Vec<usize> = Vec::new();
                for (name, lane, offset) in &held {
                    let found = self
                        .clips
                        .iter()
                        .position(|c| &c.name == name)
                        .filter(|i| !taken.contains(i))
                        .or_else(|| {
                            self.clips.iter().enumerate().find_map(|(i, c)| {
                                (!taken.contains(&i)
                                    && &c.lane == lane
                                    // **A frame, not an epsilon.** An offset
                                    // goes out in frames, crosses to beats and
                                    // comes back, so it drifts; two boxes on one
                                    // lane cannot be a frame apart and both be
                                    // the hand's.
                                    && (c.place.offset - offset).abs() < 1.0)
                                    .then_some(i)
                            })
                        });
                    if let Some(i) = found {
                        taken.push(i);
                    }
                }
                self.selected = taken;
                true
            }
            // **The track automations**: rows of their own under the lanes
            // they name, replaced whole like every other list here.
            "curves" => {
                self.curves = parse_curves(&parse::as_array_props("curves", v));
                // A curve payload states a height on every row for the reason a
                // lane payload does, so a reader who zoomed one must not lose it
                // to the next point somebody drags.
                self.zoom_rows();
                self.bodies = self.rebuilt(&self.curves, &self.layers);
                true
            }
            // **The clip envelopes**: layers inside the boxes they name.
            "layers" => {
                self.layers = parse_layers(&parse::as_array_props("layers", v));
                self.bodies = self.rebuilt(&self.curves, &self.layers);
                true
            }
            // The break-points of every curve there is, in one list: a curve
            // the list says nothing about is emptied, because the payload is
            // the whole of them and its own inverse is what was there.
            "points" => {
                let points = parse_points(&parse::as_array_props("points", v));
                for (name, body) in &mut self.bodies {
                    let flat = points.get(name).cloned().unwrap_or_default();
                    body.set("points", &Value::from(flat));
                }
                true
            }
            "layer" => {
                self.layer = v.as_str().and_then(named);
                true
            }
            "hidden" => {
                self.hidden = names_of(v);
                true
            }
            "loops" => {
                self.loops = names_of(v);
                true
            }
            "rates" => {
                self.rates = parse_rates(&parse::as_array_props("rates", v));
                true
            }
            // **The spans a join is drawn from**, replaced whole like every
            // other list here.
            "segments" => {
                self.segments = parse_segments(&parse::as_array_props("segments", v));
                true
            }
            // **Where each track's level is read from.** Set like every other
            // list here, and it has to be: the buses are allocated when a track
            // reaches the server, so a track made *after* the window opened --
            // a double click on the header -- names buses this element was
            // never built with. Read only at construction, its strip never
            // appeared, and since the strip's width is what the header lays the
            // name out against, the name spread over the space the meters
            // should have had.
            "meters" => {
                self.meters = parse_meters(&parse::as_array_props("meters", v));
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
}
