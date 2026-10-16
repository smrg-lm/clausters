//! **The layer stack of a signal picture**: what is drawn on one body, in what
//! order, at what weight, and which of them the vertical belongs to.
//!
//! A signal view draws several things on one rectangle -- the envelope, the
//! level inside it, the reconstruction over both, a loudness curve on a scale
//! of its own, a time-frequency texture under all of them. Which ones, and how
//! they sit over each other, used to be a *set* ([`Measures`]) whose order was
//! the type's own: the envelope under the level because an envelope is the
//! outer shape, and nothing to choose. That is right as a default and wrong as
//! a rule -- a spectrogram with the wave drawn over it is the same picture with
//! a different bottom, and there is no type order that decides between a
//! texture and a curve.
//!
//! So the stack is **declared**: [`Stack`] is the layers back to front, each
//! one a [`Layer`] saying what it paints, whether it is drawn, at what alpha,
//! and which vertical it is read on. The three questions this answers are the
//! whole of it, and they are the three a layered picture has:
//!
//! - **Order** -- the order the layers are written in, back to front, so what
//!   is under what is the author's statement rather than an inference from
//!   what each layer happens to be. [`Stack::drawn`] is that order, and
//!   [`Layer::visible`] / [`Layer::solo`] are how a stack is read while it is
//!   being built: hide one, or sound out one by itself, without rewriting the
//!   list.
//! - **The vertical** -- a layer either maps through the body's own axis
//!   ([`Vertical::Axis`]) or normalizes into the box it is given
//!   ([`Vertical::Box`], bringing a scale of its own, as the loudness curve
//!   does). The layers on the axis are what the y ruler and the cursor
//!   read-out report, so they must all measure the **same** quantity:
//!   [`Stack::axis_domain`] refuses a stack where two layers claim the axis
//!   for two different domains, which is the one way this question can be got
//!   wrong.
//! - **The alpha** -- [`Layer::alpha`] is a layer's own weight, multiplying the
//!   ink it is drawn in. It is a property of the layer rather than of the
//!   widget (whose `opacity` prop fades the whole subtree, chrome included),
//!   because what a translucent stack is for is reading one picture *through*
//!   another.
//!
//! **What is still not here is a second field.** The stack is inside the one
//! element for a recorded reason: every view of a signal paints its field
//! before it draws, so two elements on one rectangle are not layers -- the
//! second is a lid. One body, one axis, one ruler, one selection, one playhead,
//! one upload, and this list.

use serde_json::Value;

use clausters_core::loudness;

/// **What a column measures.** These are pictures of one span, not of
/// different sources: `Peak` is the extent the signal reached (the min/max
/// envelope every editor draws), `Rms` the level it held there -- the body an
/// editor draws *inside* that envelope -- and `Signal` what the waveform did
/// **between** the samples. One function draws any of them, placed once per
/// measure, which is what lets a view show several without a second renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Measure {
    /// The min/max envelope: what the signal reached.
    #[default]
    Peak,
    /// The symmetric body about zero at `sqrt(mean square)`: what it held.
    Rms,
    /// **The reconstruction**: the band-limited signal the samples are of,
    /// drawn between them once they are separate points, with the peaks that
    /// leave full scale marked.
    ///
    /// It is a **layer over** the sample picture and never a replacement for
    /// it, which is the whole of why it is a measure: the samples' own
    /// envelope and polyline are continuous across every zoom, and this is
    /// drawn on top of them in its own ink, so nothing about the picture
    /// *changes* when the reconstruction becomes available -- something is
    /// added. iZotope RX, the one editor that draws this at all, states the
    /// same shape: the analog waveform is plotted "in red under digital sample
    /// values (blue)". Replacing the one with the other is what makes a
    /// waveform's amplitude appear to jump at a zoom threshold, since the two
    /// differ by a fixed amount that is the signal's own inter-sample content.
    Signal,
    /// **The momentary loudness**: what a meter would have read at each point
    /// of the take, over the 400 ms up to it (ITU-R BS.1770 and EBU R 128).
    ///
    /// The one measure drawn on an axis of its own. Peak, level and
    /// reconstruction all measure the signal's *amplitude* and share the
    /// picture's vertical; loudness is in LU, so this layer brings its own
    /// scale and its own ruler and is read against the target line rather than
    /// against the samples it crosses.
    Momentary,
    /// **The short-term loudness**: the same reading over the 3 s up to each
    /// point, which is the curve a loudness track plots.
    Short,
}

impl Measure {
    /// The wire name, or `None` for one this build does not know -- which reads
    /// as "the prop was not set" rather than as an error, the protocol's own
    /// posture for an unknown value.
    pub fn parse(name: &str) -> Option<Measure> {
        match name {
            "peak" => Some(Measure::Peak),
            "rms" => Some(Measure::Rms),
            "signal" => Some(Measure::Signal),
            "momentary" => Some(Measure::Momentary),
            "short" => Some(Measure::Short),
            _ => None,
        }
    }

    /// The name this measure answers `/gui_query` with.
    pub fn name(self) -> &'static str {
        match self {
            Measure::Peak => "peak",
            Measure::Rms => "rms",
            Measure::Signal => "signal",
            Measure::Momentary => "momentary",
            Measure::Short => "short",
        }
    }

    /// The loudness window this measure reads, in seconds -- `None` for the
    /// measures of amplitude, which is also the test for *which axis a layer is
    /// drawn on*: a measure with a window is in LU and brings its own scale.
    pub fn loudness_window(self) -> Option<f64> {
        match self {
            Measure::Momentary => Some(loudness::MOMENTARY_SECONDS),
            Measure::Short => Some(loudness::SHORT_TERM_SECONDS),
            _ => None,
        }
    }

    /// Whether this measure is read in LU rather than in the picture's own
    /// vertical.
    pub fn is_loudness(self) -> bool {
        self.loudness_window().is_some()
    }
}

/// **What a view measures**, which may be more than one thing: the classic
/// editor picture is the envelope with the level body drawn *inside* it.
///
/// It is a set on the element rather than a stack of elements, and that is the
/// correction the first attempt earned: every signal picture paints its own
/// field before it draws (a heavy view's `view_field`, a plot's `track`), so two
/// pictures on one rectangle are not layers -- the second one's field hides the
/// first. Layering happens *inside* one body or not at all, which is also what
/// the picture wants: one field, one axis, one ruler, one selection, one
/// playhead, one upload.
///
/// The order is the type's own, and there is nothing to choose: the envelope is
/// the outer shape and the level lives inside it, so `Peak` draws first
/// whatever order the wire named them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measures(u8);

impl Default for Measures {
    /// **The envelope, and the reconstruction over it.** The reconstruction
    /// only draws where the samples are separate points, so at every other zoom
    /// the default is the envelope every editor draws; where they are, it is
    /// the dots and the signal through them rather than straight segments --
    /// the default iZotope RX ships, and the picture a reader zooming in to the
    /// samples is asking for. A view that wants the bare samples says
    /// `measure: "peak"`.
    fn default() -> Self {
        Measures(Measures::of(Measure::Peak).0 | Measures::of(Measure::Signal).0)
    }
}

impl Measures {
    /// Every measure there is, in drawing order -- back to front.
    pub const ALL: [Measure; 5] = [
        Measure::Peak,
        Measure::Rms,
        Measure::Signal,
        Measure::Momentary,
        Measure::Short,
    ];

    /// The set holding exactly one measure.
    pub fn of(m: Measure) -> Self {
        Measures(1 << m as u8)
    }

    /// The empty set -- what a stack with no measure in it folds from.
    pub fn none() -> Self {
        Measures(0)
    }

    /// This set, plus `m`.
    pub fn with(self, m: Measure) -> Self {
        Measures(self.0 | Measures::of(m).0)
    }

    /// The wire form: measure names separated by spaces (`"peak"`,
    /// `"peak rms"`). `None` when a name is one this build does not know or the
    /// list is empty -- which reads as "the prop was not set" rather than as an
    /// error, the protocol's own posture, and keeps a view that names nothing
    /// drawing something.
    pub fn parse(names: &str) -> Option<Self> {
        let mut set = Measures(0);
        for name in names.split_whitespace() {
            set.0 |= Measures::of(Measure::parse(name)?).0;
        }
        (set.0 != 0).then_some(set)
    }

    /// Whether `m` is drawn.
    pub fn has(self, m: Measure) -> bool {
        self.0 & Measures::of(m).0 != 0
    }

    /// The measures drawn, in drawing order.
    pub fn iter(self) -> impl Iterator<Item = Measure> {
        Measures::ALL.into_iter().filter(move |m| self.has(*m))
    }

    /// What `/gui_query` answers -- the wire form this parsed from.
    pub fn name(self) -> String {
        self.iter().map(Measure::name).collect::<Vec<_>>().join(" ")
    }
}

/// **What one layer paints.**
///
/// The two arms are the two kinds of picture a signal body can carry: a
/// measurement of the signal against time (the envelope, the level, the
/// reconstruction, a loudness curve) and the time-frequency texture. They are
/// arms of one enum rather than two lists because a stack's order runs across
/// both -- a wave over a spectrogram and a spectrogram over a wave are both
/// pictures somebody asks for, and neither is a special case of the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    /// One measure of the signal against time.
    Measure(Measure),
    /// The short-time Fourier transform, sampled as a texture -- the layer that
    /// is a *picture* rather than a curve, and the only one whose pixels come
    /// off the GPU rather than out of the window's mesh.
    Spectrogram,
}

impl Paint {
    /// The wire name, or `None` for one this build does not know -- which reads
    /// as "the prop was not set" rather than as an error, the protocol's own
    /// posture for an unknown value.
    pub fn parse(name: &str) -> Option<Paint> {
        match name {
            "spectrogram" => Some(Paint::Spectrogram),
            _ => Measure::parse(name).map(Paint::Measure),
        }
    }

    /// The name this layer answers `/gui_query` with.
    pub fn name(self) -> &'static str {
        match self {
            Paint::Measure(m) => m.name(),
            Paint::Spectrogram => "spectrogram",
        }
    }

    /// **What this layer measures along the vertical** -- the question behind
    /// the axis claim, since two layers may share the body's vertical only
    /// when they mean the same quantity by it.
    pub fn domain(self) -> Domain {
        match self {
            Paint::Spectrogram => Domain::Frequency,
            Paint::Measure(m) if m.is_loudness() => Domain::Loudness,
            Paint::Measure(_) => Domain::Amplitude,
        }
    }

    /// Where this layer is read by default: on the body's own axis, unless it
    /// has a domain the axis cannot be in -- a loudness reading is in LU, so it
    /// normalizes into its box and brings the scale with it.
    pub fn vertical(self) -> Vertical {
        match self.domain() {
            Domain::Loudness => Vertical::Box,
            Domain::Amplitude | Domain::Frequency => Vertical::Axis,
        }
    }
}

/// The quantity a layer's vertical measures. Not a display unit (dBFS and
/// percent are both amplitude): what the *axis* would have to be for the layer
/// to be drawn against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// The signal's own value -- an envelope, a level, a reconstruction.
    Amplitude,
    /// Hertz: the time-frequency texture.
    Frequency,
    /// Loudness units, relative to a target (LU/LUFS).
    Loudness,
}

impl Domain {
    /// The word a diagnostic names this domain by.
    pub fn name(self) -> &'static str {
        match self {
            Domain::Amplitude => "amplitude",
            Domain::Frequency => "frequency",
            Domain::Loudness => "loudness",
        }
    }
}

/// **Which vertical a layer is drawn on**: the body's own axis, or the box it
/// is given.
///
/// `Axis` maps through the container's vertical -- the amplitude window a zoom
/// opens, the frequency window a spectrogram is at -- and reports it, so the y
/// ruler and the cursor read-out are that layer's. `Box` normalizes into the
/// rectangle with a scale of its own and reports nothing: it is what a layer
/// with a second domain does, and what a layer wanting to stay in view whatever
/// the zoom asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vertical {
    Axis,
    Box,
}

impl Vertical {
    /// The wire word, `None` for one this build does not know.
    pub fn parse(name: &str) -> Option<Vertical> {
        match name {
            "axis" => Some(Vertical::Axis),
            "box" => Some(Vertical::Box),
            _ => None,
        }
    }

    /// The word `/gui_query` answers with.
    pub fn name(self) -> &'static str {
        match self {
            Vertical::Axis => "axis",
            Vertical::Box => "box",
        }
    }
}

/// One layer of a signal picture: what it paints and how it sits in the stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    pub paint: Paint,
    /// Whether it is drawn at all. A hidden layer keeps its place in the order,
    /// so showing it again puts it back where it was rather than on top.
    pub visible: bool,
    /// **Only the soloed layers are drawn**, when any layer solos -- the mixer's
    /// own verb, and the way a stack is read while it is built: one word turns
    /// everything else off without disturbing the list it is off in.
    pub solo: bool,
    /// The weight this layer's ink is drawn at, in `[0, 1]`.
    pub alpha: f32,
    /// The vertical it is read on, when it says so. `None` takes the paint's
    /// own ([`Paint::vertical`]), which is what almost every layer does.
    pub y: Option<Vertical>,
}

impl Layer {
    /// A layer at its defaults: drawn, opaque, on the vertical its paint
    /// implies.
    pub fn new(paint: Paint) -> Layer {
        Layer {
            paint,
            visible: true,
            solo: false,
            alpha: 1.0,
            y: None,
        }
    }

    /// Whether this layer is at every default, which is what decides whether
    /// the stack can be written as a list of bare names.
    pub fn is_plain(&self) -> bool {
        self.visible && !self.solo && self.alpha == 1.0 && self.y.is_none()
    }

    /// The vertical this layer is drawn on: what it claimed, else its paint's.
    pub fn vertical(&self) -> Vertical {
        self.y.unwrap_or_else(|| self.paint.vertical())
    }

    /// The measure this layer draws, if it draws one.
    pub fn measure(&self) -> Option<Measure> {
        match self.paint {
            Paint::Measure(m) => Some(m),
            Paint::Spectrogram => None,
        }
    }

    /// Reads one layer off the wire: a bare name, or an object naming the paint
    /// under `draw` and whatever it says about itself.
    fn parse(v: &Value) -> Option<Layer> {
        if let Some(name) = v.as_str() {
            return Paint::parse(name).map(Layer::new);
        }
        let obj = v.as_object()?;
        let paint = Paint::parse(obj.get("draw").and_then(Value::as_str)?)?;
        let mut layer = Layer::new(paint);
        if let Some(a) = obj.get("alpha").and_then(Value::as_f64) {
            layer.alpha = (a as f32).clamp(0.0, 1.0);
        }
        if let Some(b) = obj.get("visible").and_then(truthy) {
            layer.visible = b;
        }
        if let Some(b) = obj.get("solo").and_then(truthy) {
            layer.solo = b;
        }
        if let Some(y) = obj.get("y").and_then(Value::as_str) {
            layer.y = Vertical::parse(y);
        }
        Some(layer)
    }

    /// The wire form: the bare name where everything is at its default, else
    /// the object that spells what is not.
    pub fn to_value(self) -> Value {
        if self.is_plain() {
            return Value::String(self.paint.name().into());
        }
        let mut obj = serde_json::Map::new();
        obj.insert("draw".into(), Value::String(self.paint.name().into()));
        if self.alpha != 1.0 {
            obj.insert("alpha".into(), json_number(self.alpha));
        }
        if !self.visible {
            obj.insert("visible".into(), Value::Bool(false));
        }
        if self.solo {
            obj.insert("solo".into(), Value::Bool(true));
        }
        if let Some(y) = self.y {
            obj.insert("y".into(), Value::String(y.name().into()));
        }
        Value::Object(obj)
    }
}

/// **The layers of one picture, back to front.**
///
/// Ordered, because the order is the author's statement; a list rather than a
/// set, because a paint may reasonably appear twice (two loudness curves at two
/// weights is a picture somebody draws) and because a set has no front.
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    layers: Vec<Layer>,
}

impl Default for Stack {
    /// **The envelope, and the reconstruction over it**: at the zooms where
    /// the samples are separate points the
    /// reconstruction draws through them, and everywhere else it draws nothing
    /// and the envelope is what every editor shows. A view that wants the bare
    /// samples says so.
    fn default() -> Self {
        Stack::of(&[Measure::Peak, Measure::Signal])
    }
}

impl Stack {
    /// The stack drawing these measures, in this order, at their defaults.
    pub fn of(measures: &[Measure]) -> Stack {
        Stack {
            layers: measures
                .iter()
                .map(|m| Layer::new(Paint::Measure(*m)))
                .collect(),
        }
    }

    /// The stack of these layers, as given.
    pub fn from_layers(layers: Vec<Layer>) -> Stack {
        Stack { layers }
    }

    /// Every layer, drawn or not, back to front -- the list as it was declared,
    /// which is what a query answers and what a hidden layer keeps its place
    /// in.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Whether any layer solos, in which case only the soloed ones are drawn.
    pub fn soloing(&self) -> bool {
        self.layers.iter().any(|l| l.solo && l.visible)
    }

    /// **The layers actually drawn**, back to front: the visible ones, or the
    /// soloed ones where anything solos.
    pub fn drawn(&self) -> impl DoubleEndedIterator<Item = &Layer> {
        let solo = self.soloing();
        self.layers
            .iter()
            .filter(move |l| l.visible && (!solo || l.solo))
    }

    /// The measures drawn, back to front -- what a renderer placed once per
    /// measure walks.
    pub fn drawn_measures(&self) -> impl DoubleEndedIterator<Item = (Measure, f32)> + '_ {
        self.drawn()
            .filter_map(|l| l.measure().map(|m| (m, l.alpha)))
    }

    /// **The set of measures drawn**, for the one question that is about the
    /// stack rather than about a layer: a straight line between two samples
    /// exists because nothing better is available, so the sample layer drops it
    /// where the reconstruction is drawn over it.
    pub fn measures(&self) -> Measures {
        self.drawn()
            .filter_map(Layer::measure)
            .fold(Measures::none(), |set, m| set.with(m))
    }

    /// Whether a drawn layer paints `paint`.
    pub fn has(&self, paint: Paint) -> bool {
        self.drawn().any(|l| l.paint == paint)
    }

    /// Whether any drawn layer measures loudness -- the one measure that needs
    /// something computed before it can be drawn.
    pub fn has_loudness(&self) -> bool {
        self.drawn()
            .filter_map(Layer::measure)
            .any(Measure::is_loudness)
    }

    /// The alpha the topmost drawn layer painting `paint` is at, or `None` when
    /// none does.
    pub fn alpha_of(&self, paint: Paint) -> Option<f32> {
        self.drawn()
            .rev()
            .find(|l| l.paint == paint)
            .map(|l| l.alpha)
    }

    /// **What the body's vertical measures**, or an error naming the two layers
    /// that disagree about it.
    ///
    /// The layers on [`Vertical::Axis`] share the container's y -- its window,
    /// its ruler and its read-out -- so they must all mean the same quantity by
    /// it. Several amplitude measures on one axis is the ordinary picture and
    /// no claim at all; a spectrogram and a waveform both asking for it is the
    /// error, and it is an error rather than a silent winner because whichever
    /// one lost would be drawn on a scale that is not its own, which is a
    /// picture that lies.
    ///
    /// `None` is a stack where nothing is on the axis -- every layer in its own
    /// box -- and the axis is then the element's own to state.
    pub fn axis_domain(&self) -> Result<Option<Domain>, String> {
        let mut claimed: Option<(Domain, Paint)> = None;
        for layer in self.drawn().filter(|l| l.vertical() == Vertical::Axis) {
            let domain = layer.paint.domain();
            match claimed {
                Some((was, by)) if was != domain => {
                    return Err(format!(
                        "two layers claim the vertical axis for different domains: \
                         `{}` measures {} and `{}` measures {} -- one of them takes `y: \"box\"`",
                        by.name(),
                        was.name(),
                        layer.paint.name(),
                        domain.name()
                    ));
                }
                Some(_) => {}
                None => claimed = Some((domain, layer.paint)),
            }
        }
        Ok(claimed.map(|(d, _)| d))
    }

    /// **The wire form.** A stack whose layers are all at their defaults is the
    /// space-separated list of names it was most likely written as (`"peak
    /// rms"`), so the common case round-trips through the word list the
    /// `measure` prop has always been; anything else is the JSON array, which
    /// is what it has to be.
    pub fn to_wire(&self) -> String {
        if self.layers.iter().all(Layer::is_plain) {
            return self
                .layers
                .iter()
                .map(|l| l.paint.name())
                .collect::<Vec<_>>()
                .join(" ");
        }
        Value::Array(self.layers.iter().map(|l| l.to_value()).collect()).to_string()
    }

    /// The canonical array form, retaining every layer's settings.
    pub fn to_value(&self) -> Value {
        Value::Array(self.layers.iter().map(|layer| layer.to_value()).collect())
    }

    /// Read an author's stack strictly; the host's tolerant parser remains
    /// available for forward-compatible GuiDefs.
    pub fn checked(value: &Value) -> Result<Self, String> {
        let value = match value {
            Value::String(s) => serde_json::from_str::<Value>(s)
                .ok()
                .filter(Value::is_array)
                .unwrap_or_else(|| {
                    Value::Array(
                        s.split_whitespace()
                            .map(|n| Value::String(n.into()))
                            .collect(),
                    )
                }),
            value => value.clone(),
        };
        let entries = value
            .as_array()
            .ok_or("a layer stack is an array or a word list")?;
        if entries.is_empty() {
            return Err("a signal view measures something (peak, rms, signal, momentary, short, spectrogram)".into());
        }
        for entry in entries {
            let name = entry
                .as_str()
                .or_else(|| entry.get("draw").and_then(Value::as_str))
                .ok_or("a layer names what it draws")?;
            if Paint::parse(name).is_none() {
                return Err(format!(
                    "unknown measure '{name}' (peak, rms, signal, momentary, short, spectrogram)"
                ));
            }
            if let Some(y) = entry.get("y")
                && y.as_str().and_then(Vertical::parse).is_none()
            {
                return Err("a layer's y is 'axis' or 'box'".into());
            }
            for flag in ["visible", "solo"] {
                if let Some(value) = entry.get(flag)
                    && truthy(value).is_none()
                {
                    return Err(format!("a layer's {flag} is a boolean or number"));
                }
            }
            if let Some(alpha) = entry.get("alpha")
                && alpha
                    .as_f64()
                    .is_none_or(|a| !a.is_finite() || !(0.0..=1.0).contains(&a))
            {
                return Err("a layer's alpha is between 0 and 1".into());
            }
        }
        let stack = Self::parse(&value).ok_or("not a layer stack")?;
        stack.axis_domain()?;
        Ok(stack)
    }

    /// Change every occurrence of a named layer, preserving its place and
    /// settings. Showing an absent layer appends it; other edits require it.
    pub fn edit(&self, name: &str, verb: &str, alpha: Option<f64>) -> Result<Self, String> {
        let paint = Paint::parse(name).ok_or_else(|| format!("unknown layer '{name}'"))?;
        let mut layers = self.layers.clone();
        if !layers.iter().any(|layer| layer.paint == paint) {
            if verb != "show_layer" {
                return Err(format!("layer '{name}' is not in the stack"));
            }
            layers.push(Layer::new(paint));
        }
        for layer in layers.iter_mut().filter(|layer| layer.paint == paint) {
            match verb {
                "show_layer" => layer.visible = true,
                "hide_layer" => layer.visible = false,
                "solo_layer" => layer.solo = !layer.solo,
                "layer_alpha" => {
                    let alpha = alpha
                        .filter(|a| a.is_finite() && (0.0..=1.0).contains(a))
                        .ok_or("a layer's alpha is between 0 and 1")?;
                    layer.alpha = alpha as f32;
                }
                _ => return Err(format!("unknown layer verb '{verb}'")),
            }
        }
        let stack = Self::from_layers(layers);
        stack.axis_domain()?;
        Ok(stack)
    }

    /// **Reads a stack off the wire**, in either form: the space-separated
    /// names (`"peak rms"`, back to front) or the array, whose entries are
    /// names or objects.
    ///
    /// `None` where nothing legible came back -- an empty list, or a list of
    /// names this build does not know. That reads as "the prop was not set"
    /// rather than as an error, which is the protocol's posture everywhere
    /// else; a layer this build does not have is dropped and the rest are
    /// drawn, so a def written for a newer host still shows a picture.
    pub fn parse(v: &Value) -> Option<Stack> {
        let layers: Vec<Layer> = match v {
            Value::String(s) => match serde_json::from_str::<Value>(s) {
                // A JSON array that arrived as a string, which is how a
                // `/gui_set` carries a structure: the bpf's points travel the
                // same way.
                Ok(inner) if inner.is_array() => return Stack::parse(&inner),
                _ => s
                    .split_whitespace()
                    .filter_map(Paint::parse)
                    .map(Layer::new)
                    .collect(),
            },
            Value::Array(items) => items.iter().filter_map(Layer::parse).collect(),
            _ => return None,
        };
        (!layers.is_empty()).then_some(Stack { layers })
    }
}

/// A truthy wire value: a bool, or a number that is not zero -- the same
/// leniency every other flag on this wire has.
fn truthy(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|x| x != 0.0),
        _ => None,
    }
}

/// A weight as a JSON number, rounded to a thousandth.
///
/// A layer's alpha is a display weight and three decimals is finer than an eye
/// reads; what the rounding is actually for is the **answer**, since the f32
/// a client sent comes back as `0.699999988079071` in an f64 field and a query
/// that reports what was set has to read like it.
fn json_number(x: f32) -> Value {
    let rounded = (x as f64 * 1000.0).round() / 1000.0;
    serde_json::Number::from_f64(rounded).map_or(Value::Null, Value::Number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(s: &str) -> Stack {
        Stack::parse(&Value::String(s.into())).expect("a legible stack")
    }

    /// The word list is the order, back to front -- not the type's own. That is
    /// the whole of the first question: what is under what is written down.
    #[test]
    fn the_wire_order_is_the_drawing_order() {
        let s = stack("rms peak");
        let drawn: Vec<&str> = s.drawn().map(|l| l.paint.name()).collect();
        assert_eq!(drawn, ["rms", "peak"]);
        assert_eq!(s.to_wire(), "rms peak");
        // And the same names the other way round are the other picture.
        assert_eq!(
            stack("peak rms")
                .drawn()
                .map(|l| l.paint.name())
                .collect::<Vec<_>>(),
            ["peak", "rms"]
        );
    }

    /// A stack with anything said about a layer travels as the array, and comes
    /// back as what it was.
    #[test]
    fn a_stack_round_trips_through_the_wire() {
        let json = r#"["spectrogram",{"draw":"peak","alpha":0.5},{"draw":"rms","y":"box"}]"#;
        let s = Stack::parse(&serde_json::from_str::<Value>(json).unwrap()).unwrap();
        assert_eq!(s.layers().len(), 3);
        assert_eq!(s.alpha_of(Paint::Spectrogram), Some(1.0));
        assert_eq!(s.alpha_of(Paint::Measure(Measure::Peak)), Some(0.5));
        assert_eq!(s.layers()[2].vertical(), Vertical::Box);
        let back = Stack::parse(&Value::String(s.to_wire())).unwrap();
        assert_eq!(back, s, "the query's answer is the prop it parsed from");
    }

    /// Hiding keeps the place: a layer shown again comes back where it was,
    /// rather than on top of everything.
    #[test]
    fn a_hidden_layer_keeps_its_place() {
        let json = r#"["peak",{"draw":"rms","visible":false},"signal"]"#;
        let mut s = Stack::parse(&serde_json::from_str::<Value>(json).unwrap()).unwrap();
        assert_eq!(
            s.drawn().map(|l| l.paint.name()).collect::<Vec<_>>(),
            ["peak", "signal"]
        );
        s.layers[1].visible = true;
        assert_eq!(
            s.drawn().map(|l| l.paint.name()).collect::<Vec<_>>(),
            ["peak", "rms", "signal"]
        );
    }

    /// Solo is the mixer's verb: one layer on turns the others off without
    /// touching what they say about themselves.
    #[test]
    fn solo_draws_only_what_solos() {
        let json = r#"["spectrogram",{"draw":"peak","solo":true},"rms"]"#;
        let s = Stack::parse(&serde_json::from_str::<Value>(json).unwrap()).unwrap();
        assert!(s.soloing());
        assert_eq!(
            s.drawn().map(|l| l.paint.name()).collect::<Vec<_>>(),
            ["peak"]
        );
        assert!(!s.has(Paint::Spectrogram), "and it is not drawn either");
    }

    /// The default vertical follows what a layer measures: amplitudes are on
    /// the body's axis, a loudness reading is in its box with its own scale.
    #[test]
    fn a_loudness_layer_is_in_its_box_and_the_rest_on_the_axis() {
        let s = stack("peak rms momentary");
        assert_eq!(s.layers()[0].vertical(), Vertical::Axis);
        assert_eq!(s.layers()[2].vertical(), Vertical::Box);
        assert_eq!(s.axis_domain(), Ok(Some(Domain::Amplitude)));
        assert!(s.has_loudness());
    }

    /// The axis-claim rule, which is the one way the vertical can be got
    /// wrong: two layers on it measuring two things.
    #[test]
    fn two_domains_cannot_claim_one_axis() {
        let s = stack("spectrogram peak");
        let Err(msg) = s.axis_domain() else {
            panic!("a texture and a wave on one axis is refused")
        };
        assert!(msg.contains("spectrogram") && msg.contains("peak"), "{msg}");
        // And naming the box for one of them is what makes it legal.
        let ok = Stack::parse(
            &serde_json::from_str::<Value>(r#"["spectrogram",{"draw":"peak","y":"box"}]"#).unwrap(),
        )
        .unwrap();
        assert_eq!(ok.axis_domain(), Ok(Some(Domain::Frequency)));
    }

    /// A name this build does not know is dropped rather than refused, and a
    /// stack of nothing but such names reads as a prop that was not set.
    #[test]
    fn an_unknown_layer_is_not_an_error() {
        assert_eq!(
            stack("peak wavelet")
                .drawn()
                .map(|l| l.paint.name())
                .collect::<Vec<_>>(),
            ["peak"]
        );
        assert!(Stack::parse(&Value::String("wavelet".into())).is_none());
        assert!(Stack::parse(&Value::String(String::new())).is_none());
    }

    /// A `/gui_set` carries the array as a string, the way every structure on
    /// this wire does.
    #[test]
    fn the_array_travels_as_a_string_too() {
        let s = Stack::parse(&Value::String(
            r#"["spectrogram",{"draw":"peak","alpha":0.25}]"#.into(),
        ))
        .unwrap();
        assert_eq!(s.alpha_of(Paint::Measure(Measure::Peak)), Some(0.25));
    }
}
