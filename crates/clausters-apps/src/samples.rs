//! **The window a take is drawn in**, and the measures its picture stacks --
//! what the audio editor opens ([`crate::audio`]).
//!
//! One widget, the take's picture: a `waveform` over the server buffer on its
//! own time axis, measured as a stack -- what the signal reached (`peak`) with
//! what it held inside that (`rms`) -- and edited with the catalogue's
//! three-gesture plan (a drag selects, Alt draws, Ctrl grabs one sample). The
//! picture is the catalogue's (`clausters_document::view::catalogue`); what is
//! the application's is the window it stands in, the measures it stacks, the
//! label a take is shown under and the correction a write answers with.
//!
//! # The measures are one prop, not a pile of widgets
//!
//! Every view of a signal paints its own field before it draws, so two of them
//! on one rectangle are not layers: the second hides the first. Measuring twice
//! into one body is also what keeps the rest of it one thing -- one axis, one
//! ruler, one selection, one playhead, one upload of the samples.
//!
//! # The ids are the caller's
//!
//! As in the multitrack's window, the one widget id is a running host's fact
//! and comes in as an argument.

use serde_json::{Map, Value, json};

use clausters_document::view::catalogue::{self, Waveform};

/// **The measures a signal view can stack**, in the order a reader thinks of
/// them: what the signal reached, and what it held inside that.
pub const MEASURES: [&str; 2] = ["peak", "rms"];

/// **A measure stack, checked.**
///
/// A stack is written by hand, so a silent typo is a layer that quietly does not
/// appear, and an empty one is a picture that measures nothing: both are
/// refused, with the reason.
pub fn measures<S: AsRef<str>>(stack: &[S]) -> Result<Vec<String>, String> {
    let known = MEASURES.join(", ");
    let mut out = Vec::with_capacity(stack.len());
    for name in stack {
        let name = name.as_ref();
        if !MEASURES.contains(&name) {
            return Err(format!("unknown measure '{name}' (one of {known})"));
        }
        out.push(name.to_string());
    }
    if out.is_empty() {
        return Err(format!("a signal view measures something (one of {known})"));
    }
    Ok(out)
}

/// What the header of a take is labelled: its name, or the buffer it is in when
/// it has none.
pub fn label(name: Option<&str>, buffer: i64) -> String {
    match name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("buffer {buffer}"),
    }
}

/// Everything a window over a take is composed from.
pub struct Window<'a> {
    /// The server buffer the take is in.
    pub buffer: i64,
    /// Its interleaved channel count.
    pub channels: u32,
    /// The take's name, when it has one.
    pub name: Option<&'a str>,
    /// What the picture measures, innermost last, already checked
    /// ([`measures`]).
    pub layers: &'a [String],
    /// The rate the axis reads its numbers with.
    pub rate: f64,
    /// The id of the take's widget.
    pub widget: i32,
    /// The window's title.
    pub title: &'a str,
    /// The window's size, in logical pixels.
    pub size: (i64, i64),
    /// The level meter beside the picture, once something plays the take.
    pub meter: Option<MeterAt>,
}

/// **Where the level meter is read from**, and the id it is drawn under: the
/// first of the control buses the playback's meter writes, one per channel of
/// the editor's bus -- a running playback's facts, handed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeterAt {
    /// The meter widget's id.
    pub widget: i32,
    /// The first control bus.
    pub bus: i32,
    /// How many channels, one column each.
    pub channels: usize,
}

/// **The window**, as a GuiDef rooted at a `window` node.
///
/// The root carries no id: a GuiDef's id is the one its `/gui_def` names. A
/// script's own widgets are not composed here, for the multitrack's reason: they
/// are the client's objects, and a client appends them.
pub fn window(w: &Window<'_>) -> Value {
    let mut picture = catalogue::waveform(&Waveform {
        buffer: Some(w.buffer),
        channels: Some(w.channels.max(1)),
        measure: w.layers.join(" "),
        ruler: "time".into(),
        sample_rate: w.rate,
        label: label(w.name, w.buffer),
        // **Both cursors are there from the start.** The position cursor
        // stands at the take's first frame, where a play with nothing placed
        // starts; the play cursor is anchored at 0 because the counter it is
        // drawn from is the transport's position (the editor asks for that
        // clock when it opens), so it stands on the position cursor until
        // something plays and goes back there when it stops.
        cursor: Some(0.0),
        playhead_at: Some(0.0),
        ..Waveform::default()
    });
    picture.insert("id".into(), json!(w.widget));
    let mut children = vec![Value::Object(picture)];
    // **The level, beside the take**: the editor's output measured before its
    // declick, one column per channel, read off the control buses the
    // playback's meter writes. The mark that waits is the widget's own
    // ballistics.
    if let Some(meter) = w.meter {
        children.push(json!({
            "type": "meter",
            "id": meter.widget,
            "bus": meter.bus,
            "rate": "control",
            // A control bus reads linearly unless told: this one carries an
            // amplitude, read in decibels with its ladder.
            "scale": "db",
            "channels": meter.channels,
        }));
    }
    json!({
        "type": "window",
        "title": w.title,
        "w": w.size.0,
        "h": w.size.1,
        "flow": if w.meter.is_some() { "row" } else { "col" },
        // **The space bar and `L` are the application's**: it plays its own
        // take through its own playback, so the host's monitor stays out.
        "plays": true,
        "children": children,
    })
}

/// **What a widget of this window is corrected with**: read the take again.
///
/// The picture is the server's buffer. A stroke needs nothing from here -- the
/// host wrote those cells itself and its picture moved with them -- but a write
/// made from the other side (an undo, a script) is one the host never saw, so
/// the window would keep drawing the stroke until some other reason made it
/// resolve the source again. `reload` makes the element forget what it resolved
/// and read it on the next pass. The generation pairs `/gui_ack` carries would
/// say the same more cheaply, but no client sends one and the host acts on none,
/// so this is the door that is open.
pub fn props(_widget: i32) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("reload".into(), json!(1));
    out
}

/// [`measures`] over JSON: `{"stack": [...]}` answers `{"layers": [...]}`, or
/// `{"error"}` naming what is wrong.
pub fn measures_json(request: &str) -> String {
    let stack: Vec<String> = serde_json::from_str::<Value>(request)
        .ok()
        .and_then(|r| r.get("stack").cloned())
        .and_then(|s| serde_json::from_value(s).ok())
        .unwrap_or_default();
    match measures(&stack) {
        Ok(layers) => json!({ "layers": layers }).to_string(),
        Err(error) => json!({ "error": error }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose(name: Option<&str>) -> Value {
        let layers = vec!["peak".to_string(), "rms".to_string()];
        window(&Window {
            buffer: 3,
            channels: 2,
            name,
            layers: &layers,
            rate: 48_000.0,
            widget: 9,
            title: "take",
            meter: None,
            size: (1000, 520),
        })
    }

    /// **One waveform in a column**, numbered with the id it was handed, on a
    /// time ruler, with the three gestures and both measures.
    #[test]
    fn the_window_is_one_waveform_over_the_take() {
        let def = compose(Some("glide"));
        assert_eq!(def["type"], "window");
        assert_eq!(def["flow"], "col");
        assert_eq!(
            (def["w"].clone(), def["h"].clone()),
            (json!(1000), json!(520))
        );
        let children = def["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        let take = &children[0];
        assert_eq!(take["type"], "signal");
        assert_eq!(take["id"], 9);
        assert_eq!(take["buffer"], 3);
        assert_eq!(take["channels"], 2);
        assert_eq!(take["measure"], "peak rms");
        assert_eq!(
            take["axes"]["x"]["cursor"], 0.0,
            "the position cursor, placed"
        );
        assert_eq!(
            take["axes"]["x"]["playhead_at"], 0.0,
            "the play cursor, anchored"
        );
        assert_eq!(take["label"], "glide");
        assert_eq!(take["axes"]["x"]["unit"], "time");
        assert!(
            take["axes"]["x"].get("tempo").is_none(),
            "a take holds no tempo"
        );
        assert_eq!(
            take["gestures"],
            json!({"drag": "select", "alt": "draw", "ctrl": "sample"})
        );
    }

    /// A take with no name is shown under the buffer it is in.
    #[test]
    fn a_nameless_take_is_labelled_by_its_buffer() {
        assert_eq!(compose(None)["children"][0]["label"], "buffer 3");
        assert_eq!(compose(Some(""))["children"][0]["label"], "buffer 3");
    }

    /// **A correction is "read it again"**, whichever widget it names.
    #[test]
    fn a_correction_reloads_the_picture() {
        assert_eq!(Value::Object(props(9)), json!({"reload": 1}));
    }

    /// A typo and an empty stack are refused with their reason; a good stack
    /// comes back in the order it was written.
    #[test]
    fn a_measure_stack_is_checked() {
        assert_eq!(measures(&["rms", "peak"]).unwrap(), ["rms", "peak"]);
        let typo = measures(&["peak", "rsm"]).unwrap_err();
        assert!(
            typo.contains("'rsm'") && typo.contains("peak, rms"),
            "{typo}"
        );
        let empty = measures::<&str>(&[]).unwrap_err();
        assert!(empty.contains("measures something"), "{empty}");
        assert_eq!(
            measures_json(r#"{"stack": ["peak"]}"#),
            r#"{"layers":["peak"]}"#
        );
        assert!(measures_json(r#"{"stack": []}"#).contains("error"));
    }
}
