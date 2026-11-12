//! The multitrack's beat<->second time map, as an object a page holds.

#[cfg(target_arch = "wasm32")]
use clausters_core::tempomap::Curve;
#[cfg(target_arch = "wasm32")]
use clausters_core::tempomap::Extent;
#[cfg(target_arch = "wasm32")]
use clausters_core::tempomap::Shape;
#[cfg(target_arch = "wasm32")]
use clausters_core::tempomap::TempoMap;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// The multitrack's beat<->second time map, the JS face of
/// [`clausters_core::tempomap::TempoMap`].
///
/// A beat is a logical coordinate, not a unit of time; this is the function
/// that turns one into the other under a tempo that changes along the multitrack.
/// It is pure -- it knows nothing of *now* -- so an editor, an offline render
/// and a live clock share one, and there is a single implementation of the
/// integral behind all of them.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = TempoMap)]
pub struct JsTempoMap(pub(crate) TempoMap);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = TempoMap)]
impl JsTempoMap {
    /// A map of one constant-tempo segment (beat 0 at second 0). A tempo that
    /// is not finite and positive falls back to 1.0.
    #[wasm_bindgen(constructor)]
    pub fn new(tempo: f64) -> JsTempoMap {
        JsTempoMap(TempoMap::new(tempo))
    }

    /// A map of one constant-tempo segment with `baseBeats` falling on
    /// `baseSeconds` -- the affine triple a running clock already holds, so
    /// adopting a map changes no result. `undefined` on invalid arguments.
    pub fn anchored(tempo: f64, base_beats: f64, base_seconds: f64) -> Option<JsTempoMap> {
        TempoMap::anchored(tempo, base_beats, base_seconds)
            .ok()
            .map(JsTempoMap)
    }

    /// **A map from a multitrack's authored tempo entries** -- the JSON array a
    /// document's `tempo` list is, plus the tempo a multitrack that never said one
    /// leaves to its reader. `undefined` when the text is not such a list.
    ///
    /// The bridge a reader of a document would otherwise take three decisions
    /// to write: a ramp reaches the *next* entry, the default is prepended when
    /// the first entry is past beat 0, and no entries at all is the default
    /// alone. Each entry is `{beats, tempo, ramp}` with the tempo in beats
    /// **per second**, as every tempo here is -- a document writing beats per
    /// minute divides once, where it reads its own field.
    #[wasm_bindgen(js_name = fromChanges)]
    pub fn from_changes(changes: &str, default_tempo: f64) -> Option<JsTempoMap> {
        #[derive(serde::Deserialize)]
        struct Change {
            beats: f64,
            tempo: f64,
            #[serde(default)]
            ramp: bool,
        }
        let parsed: Vec<Change> = serde_json::from_str(changes).ok()?;
        let changes: Vec<clausters_core::tempomap::TempoChange> = parsed
            .into_iter()
            .map(|c| clausters_core::tempomap::TempoChange {
                beats: c.beats,
                tempo: c.tempo,
                ramp: c.ramp,
            })
            .collect();
        TempoMap::from_changes(&changes, default_tempo)
            .ok()
            .map(JsTempoMap)
    }

    /// An independent copy -- a fork, for when two tempi should stop being one.
    /// Handing a map to a clock does **not** copy: a clock adopts what it is
    /// given, which is what lets two clocks read one multitrack.
    #[wasm_bindgen(js_name = copy)]
    pub fn copy(&self) -> JsTempoMap {
        JsTempoMap(self.0.clone())
    }

    /// The map's edit count, bumped by every write that lands.
    ///
    /// What a holder of a **shared** map compares to learn that something
    /// moved: every reader re-evaluates from the map itself, so this is all
    /// the machinery a second clock on one map needs.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> f64 {
        // f64 rather than u64: a JS number holds every edit count anyone will
        // reach, and a BigInt at this boundary would make a plain `!==`
        // comparison the caller's problem.
        self.0.version() as f64
    }

    /// The map written out as JSON -- its breakpoints, without the derived
    /// seconds.
    #[wasm_bindgen(js_name = dump)]
    pub fn dump(&self) -> String {
        serde_json::to_string(&self.0).unwrap_or_default()
    }

    /// A map read back from the JSON [`Self::dump`] writes. `undefined` when
    /// the text is not a map this client could have written -- the breakpoints
    /// are replayed through the ordinary writers, so every rule a live gesture
    /// obeys is checked here.
    #[wasm_bindgen(js_name = load)]
    pub fn load(json: &str) -> Option<JsTempoMap> {
        serde_json::from_str::<TempoMap>(json).ok().map(JsTempoMap)
    }

    /// **The time map**: the second beat `b` falls on.
    #[wasm_bindgen(js_name = secsAt)]
    pub fn secs_at(&self, b: f64) -> f64 {
        self.0.secs_at(b)
    }

    /// The inverse: the beat falling on second `s`.
    #[wasm_bindgen(js_name = beatsAt)]
    pub fn beats_at(&self, s: f64) -> f64 {
        self.0.beats_at(s)
    }

    /// The tempo (beats per second) in effect at beat `b`.
    #[wasm_bindgen(js_name = tempoAt)]
    pub fn tempo_at(&self, b: f64) -> f64 {
        self.0.tempo_at(b)
    }

    /// How long the stretch from `b0` to `b1` lasts, in seconds -- the only
    /// correct way to turn a length in beats into a length in time, since the
    /// same span lasts differently depending on where it sits.
    #[wasm_bindgen(js_name = spanSecs)]
    pub fn span_secs(&self, b0: f64, b1: f64) -> f64 {
        self.0.span_secs(b0, b1)
    }

    /// How many beats fit in `secs` seconds starting at beat `b0`.
    #[wasm_bindgen(js_name = spanBeats)]
    pub fn span_beats(&self, b0: f64, secs: f64) -> f64 {
        self.0.span_beats(b0, secs)
    }

    /// Appends a constant-tempo change at beat `b`. Returns whether it was
    /// taken; a refused change leaves the map as it was.
    pub fn push(&mut self, b: f64, tempo: f64) -> bool {
        self.0.push(b, tempo).is_ok()
    }

    /// Writes a tempo ramp over `[fromBeats, toBeats]` from `fromTempo` to
    /// `toTempo`, holding `toTempo` after it.
    pub fn ramp(&mut self, from_beats: f64, to_beats: f64, from_tempo: f64, to_tempo: f64) -> bool {
        self.0
            .ramp(from_beats, to_beats, from_tempo, to_tempo)
            .is_ok()
    }

    /// {@link TempoMap.ramp} in an explicit shape: `shape` is the envelope
    /// shape number (1 linear, 2 exponential, 5 a numeric curvature) and
    /// `curvature` is read only by shape 5. False for a shape number no tempo
    /// curve has, and for a ramp the map refuses.
    pub fn shaped(
        &mut self,
        from_beats: f64,
        to_beats: f64,
        from_tempo: f64,
        to_tempo: f64,
        shape: u32,
        curvature: f64,
    ) -> bool {
        match Shape::from_parts(shape, curvature) {
            Some(shape) => self
                .0
                .shaped(from_beats, to_beats, from_tempo, to_tempo, shape)
                .is_ok(),
            None => false,
        }
    }

    /// Writes a whole tempo envelope from beat `at`: `tempos` holds one more
    /// value than `extents`, and `shapes`/`curvatures` one each. `seconds`
    /// reads the extents as wall clock rather than as beats. A refused
    /// envelope writes nothing.
    pub fn env(
        &mut self,
        at: f64,
        tempos: Vec<f64>,
        extents: Vec<f64>,
        shapes: Vec<u32>,
        curvatures: Vec<f64>,
        seconds: bool,
    ) -> bool {
        if shapes.len() != extents.len() || curvatures.len() != extents.len() {
            return false;
        }
        let mut resolved = Vec::with_capacity(shapes.len());
        for (shape, curvature) in shapes.iter().zip(&curvatures) {
            match Shape::from_parts(*shape, *curvature) {
                Some(shape) => resolved.push(shape),
                None => return false,
            }
        }
        let unit = match seconds {
            true => Extent::Seconds,
            false => Extent::Beats,
        };
        self.0
            .write_env(at, &tempos, &extents, &resolved, unit)
            .is_ok()
    }

    /// Drops every breakpoint at or after beat `b` (never the first).
    #[wasm_bindgen(js_name = truncateFrom)]
    pub fn truncate_from(&mut self, b: f64) {
        self.0.truncate_from(b);
    }

    /// How many segments the map holds (always at least 1).
    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always false -- a map holds at least one segment by construction.
    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Segment `i` as `[beats, secs, tempo, shape, endBeats, endTempo,
    /// curvature]` -- `shape` is 0 for a constant tempo and otherwise the
    /// envelope shape number, and the three trailing fields are 0 when there is
    /// no curve. `undefined` past the end.
    pub fn segment(&self, i: usize) -> Option<Vec<f64>> {
        self.0.segments().get(i).map(|seg| {
            let (shape, end_beats, end_tempo, curvature) = match seg.curve {
                Curve::Step => (0.0, 0.0, 0.0, 0.0),
                Curve::Shaped {
                    shape,
                    end_beats,
                    end_tempo,
                } => (
                    f64::from(shape.number()),
                    end_beats,
                    end_tempo,
                    shape.curvature(),
                ),
            };
            vec![
                seg.beats, seg.secs, seg.tempo, shape, end_beats, end_tempo, curvature,
            ]
        })
    }

    /// The last segment's affine triple, `[baseBeats, baseSeconds, tempo]` --
    /// what a clock caches so reading *now* stays three float operations with
    /// no search.
    pub fn last(&self) -> Vec<f64> {
        let last = self.0.last();
        vec![last.beats, last.secs, last.tempo]
    }
}
