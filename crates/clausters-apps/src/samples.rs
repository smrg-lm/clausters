//! **The window a take is drawn in**, and the measures its picture stacks --
//! what the audio editor opens ([`crate::audio`]).
//!
//! A pane owns the time axis, with the take's signal body over the server
//! buffer, measured as a stack -- what the signal reached (`peak`) with
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
//! As in the multitrack's window, the widget ids are a running host's facts
//! and come in as arguments.

use clausters_document::view::layers::Stack;
use serde_json::{Map, Value, json};

use clausters_document::view::catalogue::{self, Waveform};

/// **The audio editor's default stack**: what the signal reached, and
/// what it held inside that. Other layers use the shared view vocabulary.
pub const MEASURES: [&str; 2] = ["peak", "rms"];

/// **A measure stack, checked.**
///
/// A stack is written by hand, so a silent typo is a layer that quietly does not
/// appear, and an empty one is a picture that measures nothing: both are
/// refused, with the reason.
pub fn measures<S: AsRef<str>>(stack: &[S]) -> Result<Vec<String>, String> {
    let value = json!(stack.iter().map(AsRef::as_ref).collect::<Vec<_>>());
    let checked = Stack::checked(&value)?;
    Ok(checked
        .layers()
        .iter()
        .map(|layer| layer.paint.name().to_string())
        .collect())
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
    /// (the shared layer validator).
    pub layers: &'a Stack,
    /// The rate the axis reads its numbers with.
    pub rate: f64,
    /// The id of the take's widget.
    pub widget: i32,
    /// The axis container id, when composed as a pane.
    pub pane: Option<i32>,
    /// The transport-position anchor; negative hides the play cursor.
    pub playhead_at: f64,
    /// The body currently in hand.
    pub layer: &'a str,
    /// The window's title.
    pub title: &'a str,
    /// The window's size, in logical pixels.
    pub size: (i64, i64),
    /// The level meter beside the picture, once something plays the take.
    pub meter: Option<MeterAt>,
    /// **A window with no chrome**: no menu bar and no tools, for a client
    /// that composes its own around it.
    pub bare: bool,
    /// The dialogs a menu entry opens, as the stack the window holds, when
    /// the caller numbered them.
    pub dialogs: Option<Value>,
    /// The channels drawn folded.
    pub folded: &'a [u32],
    /// **The spectrogram's lane, for the stacked views**: the id it is
    /// numbered with, drawn under the take's lane on the same axis. `None`
    /// is the one lane, showing whichever view `layers` is.
    pub lane: Option<i32>,
    /// **No file is open**: the window keeps its chrome and its dialogs, and
    /// where the take would be it says how to open one.
    pub empty: bool,
    /// **The take as a list of segments over files**, which the picture
    /// draws from its files in place of `buffer`: an audio editor's take
    /// lives on disk.
    pub segments: Option<&'a Value>,
}

/// What a window with no file open says where the take would be.
pub const NO_FILE: &str = "No file is open. File > New file... or File > Open file...";

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
    if w.empty {
        return dressed(
            w,
            vec![json!({"type": "label", "id": w.widget, "text": NO_FILE,
                "align": "center", "weight": 1.0})],
        );
    }
    let mut picture = catalogue::waveform(&Waveform {
        buffer: Some(w.buffer),
        channels: Some(w.channels.max(1)),
        measure: w.layers.to_wire(),
        ruler: "time".into(),
        sample_rate: w.rate,
        label: label(w.name, w.buffer),
        // The application decides whether this file reads the transport.
        // Rebuilding a view must not re-enable another file's play cursor.
        cursor: Some(0.0),
        playhead_at: Some(w.playhead_at),
        ..Waveform::default()
    });
    picture.insert("id".into(), json!(w.widget));
    picture.insert("weight".into(), json!(1.0));
    if let Some(list) = w.segments {
        picture.remove("buffer");
        picture.insert("segments".into(), list.clone());
    }
    // A take drawn from a summary leaves the host no samples to copy: the
    // editor copies it, naming the takes the span is made of.
    picture.insert("copies".into(), json!("owner"));
    if !w.folded.is_empty() {
        picture.insert("folded".into(), json!(w.folded));
    }
    // **The spectrogram's lane**, stacked under the take's: another view of the
    // same samples, a member of the take's axis (its selection, its cursor,
    // its play cursor) with a vertical of its own. It carries the time ruler,
    // at the bottom of the stack, and the take's lane carries none.
    let lane = w.lane.map(|id| {
        let mut lane = catalogue::waveform(&Waveform {
            buffer: Some(w.buffer),
            channels: Some(w.channels.max(1)),
            measure: "spectrogram".into(),
            ruler: "time".into(),
            sample_rate: w.rate,
            ..Waveform::default()
        });
        lane.insert("id".into(), json!(id));
        lane.insert("weight".into(), json!(1.0));
        if let Some(list) = w.segments {
            lane.remove("buffer");
            lane.insert("segments".into(), list.clone());
        }
        lane.insert("link".into(), json!(w.pane.unwrap_or(w.widget)));
        // A picture of the take, never a surface a hand edits samples on.
        lane.insert("editable".into(), json!(false));
        if !w.folded.is_empty() {
            lane.insert("folded".into(), json!(w.folded));
        }
        Value::Object(lane)
    });
    let ruled = lane.is_none();
    let picture = if let Some(pane) = w.pane {
        let axes = picture.remove("axes").unwrap_or(Value::Null);
        let label = picture.remove("label");
        // Only the horizontal axis moves to the pane. The body's vertical
        // scale is still its own, including the visual stack's domain.
        if let Some(y) = axes.get("y") {
            picture.insert("axes".into(), json!({"y": y}));
        }
        let mut x = axes.get("x").cloned().unwrap_or_else(|| json!({}));
        if !ruled {
            x["ruler"] = json!(false);
        }
        json!({"type": "pane", "id": pane, "weight": 1, "layer": w.layer, "label": label,
            "axes": {"x": x}, "children": [picture]})
    } else {
        if !ruled {
            picture.insert("ruler".into(), json!(false));
        }
        Value::Object(picture)
    };
    let picture = match lane {
        Some(lane) => json!({"type": "layout", "flow": "col", "margin": 0, "weight": 1,
            "children": [picture, lane]}),
        None => picture,
    };
    let mut children = vec![picture];
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
    // The take and its meter side by side, under the toolbar.
    let body = if w.meter.is_some() {
        vec![
            json!({"type": "layout", "flow": "row", "margin": 0, "weight": 1, "children": children}),
        ]
    } else {
        children
    };
    dressed(w, body)
}

/// The window around `body`: its keys and verbs, its chrome and its dialogs.
fn dressed(w: &Window<'_>, body: Vec<Value>) -> Value {
    let mut window = json!({
        "type": "window",
        "title": w.title,
        "w": w.size.0,
        "h": w.size.1,
        "flow": "col",
        // **The space bar and `L` are the application's**: it plays its own
        // take through its own playback, so the host's monitor stays out.
        "plays": true,
        // **The window coming back is told**, which is when the files it
        // reads are looked at on disk.
        "activation": true,
        "keys": [clausters_editing::verbs::AUDIO_SCOPE],
        "verbs": clausters_editing::verbs::declaration(&[(
            clausters_editing::verbs::AUDIO_SCOPE, clausters_editing::verbs::AUDIO,
        )]),
        "children": body,
    });
    if !w.bare {
        let mut menu = crate::chrome::menu(crate::chrome::App::Audio, true);
        crate::chrome::channel_entries(&mut menu, w.channels);
        crate::chrome::dress(
            &mut window,
            menu,
            crate::chrome::toolbar(crate::chrome::App::Audio, None),
            w.widget,
        );
    }
    if let (Some(stack), Some(children)) = (
        w.dialogs.clone(),
        window.get_mut("children").and_then(Value::as_array_mut),
    ) {
        children.push(stack);
    }
    window
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
    let stack = serde_json::from_str::<Value>(request)
        .ok()
        .and_then(|r| r.get("stack").cloned())
        .unwrap_or(Value::Null);
    match Stack::checked(&stack) {
        Ok(layers) => json!({ "layers": layers.to_value() }).to_string(),
        Err(error) => json!({ "error": error }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose(name: Option<&str>) -> Value {
        let layers = Stack::checked(&json!(["peak", "rms"])).unwrap();
        window(&Window {
            buffer: 3,
            channels: 2,
            name,
            layers: &layers,
            rate: 48_000.0,
            widget: 9,
            pane: None,
            playhead_at: 0.0,
            layer: "take",
            title: "take",
            meter: None,
            size: (1000, 520),
            bare: true,
            dialogs: None,
            folded: &[],
            lane: None,
            empty: false,
            segments: None,
        })
    }

    /// **One waveform in a column**, numbered with the id it was handed, on a
    /// time ruler, with the three gestures and both measures.
    #[test]
    fn the_window_is_one_waveform_over_the_take() {
        let def = compose(Some("glide"));
        assert_eq!(def["type"], "window");
        assert_eq!(def["flow"], "col");
        assert_eq!(def["keys"], json!(["audio"]));
        assert_eq!(def["verbs"]["audio"]["view_wave"]["keys"], json!(["W"]));
        assert_eq!(
            def["verbs"]["audio"]["view_spectrogram"]["keys"],
            json!(["S"])
        );
        assert_eq!(def["verbs"]["audio"]["view_both"]["keys"], json!(["B"]));
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
