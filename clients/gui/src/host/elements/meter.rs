//! `meter` -- a bus level as a column, with everything a meter is read by.
//!
//! The smallest thing that reads the **world**: one bus per channel, one read
//! per tick, nothing else. Which table that read lands in is the `rate` prop --
//! an audio bus publishes one block level, a control bus carries a value -- and
//! both are one atomic load out of the same source, so a meter costs neither a
//! message nor a recording. That is also the whole of what it declares: a
//! control-rate meter contributes its buses to the frame's stream
//! subscription, an audio-rate one contributes levels, and the window animates
//! because of that declaration rather than because a collector knew the word
//! `meter`.
//!
//! **What makes it a meter rather than a bar** is four things, and every one of
//! them is a rule the shared core owns rather than this drawing:
//!
//! - **The peak is the block's, not a sample of it.** The server walks every
//!   sample of every block and publishes the peak, held with a decay
//!   (`LEVEL_RELEASE_DB_PER_SEC`), so a reader running at a screen's rate -- a
//!   frame is a dozen blocks -- sees the transient instead of whatever sample it
//!   happened to look at. At control rate the `Meter` UGen does the same and
//!   the bus carries the answer.
//! - **A peak is held.** [`Ballistics`] keeps the loudest reading `hold`
//!   seconds and then lets it fall at `decay` decibels per second, which is the
//!   hairline across the column: the mark is still there when an eye gets to
//!   it.
//! - **The scale is decibels**, from a floor the reader states -- the 60 dB
//!   strip a mix is read on, or the dynamic range of the resolution the multitrack
//!   is rendered at (`bits`).
//! - **An over is latched.** Clipping is a handful of samples and a person is
//!   not, so the lamp over the column stays lit until a hand puts it out (a
//!   click) or the counter it watches starts again (a new pass). What counts as
//!   an over is a **run** of samples at full scale, which only something
//!   walking samples can see: the `ClipCount` UGen counts them onto a control
//!   bus and this widget differences it. With no such bus it lights on the
//!   level alone reaching the top, which is exact for exceeding full scale and
//!   blind to how many samples did.

use clausters_core::measure::{Ballistics, CLIP_CEILING, ClipLatch, floor_db_for_bits};
use serde_json::{Map, Value};

use crate::host::graphics::meters::{self, ChannelRead, MeterAxis, MeterView, Zones};
use crate::host::paint::Draw;
use crate::host::ruler::Side;
use crate::host::widget::element::{Claim, Ctx, Element, Input, Live, Needs};
use crate::host::widget::size::Natural;
use crate::host::widget::{Rate, parse};

/// How many channels a meter will read adjacent buses for. A meter of a whole
/// mix is one or two; the cap is here so a typo cannot ask for a thousand
/// columns in a cell forty pixels wide.
const MAX_CHANNELS: usize = 64;

/// The seconds a peak mark waits before it begins to fall, unless told
/// otherwise. Long enough to be read at a glance, short enough that the mark
/// still follows the music.
const HOLD: f32 = 1.5;

/// A level meter over one or more adjacent buses.
#[derive(Debug, Clone)]
pub struct Meter {
    pub bus: i32,
    /// Audio rate reads each bus's published block level; control rate reads
    /// the control bus's current value.
    pub rate: Rate,
    /// How many adjacent buses are metered, one column each.
    pub channels: usize,
    /// What the height measures.
    pub axis: MeterAxis,
    /// Seconds a peak is held before it falls; `0` draws no mark at all.
    pub hold: f32,
    /// The mark's fall, in decibels per second.
    pub decay: f32,
    /// The first control bus of the over counts (`ClipCount`), one per channel,
    /// or `None` for a meter that watches its own level.
    pub clip: Option<i32>,
    /// Which side the numbers fall on, `None` for no ladder.
    pub ruler: Option<Side>,
    /// Whether the meter writes **numbers over its columns**: the level at its
    /// foot, and how far past full scale a lit lamp went. On by default, since
    /// a lamp says that something clipped and the number says by how much,
    /// which is the question the lamp raises.
    ///
    /// Off is a **bare column**, and that is a real meter and not a degraded
    /// one: a strip of them down the edge of a track header carries no ladder
    /// and no figures, because there is no room for either and the picture is
    /// the whole of what it has to say.
    pub readout: bool,
    /// **What the level on the bus is** -- the largest sample, or the true peak
    /// of the reconstructed signal a `TruePeak` UGen writes.
    pub peak: Peak,
    /// What the columns are coloured by (the `zones` prop).
    pub zones: Zones,
    pub label: Option<String>,
    /// What each channel reads, advanced once per tick. The element's own, so
    /// a window that repaints twice does not fall twice.
    state: Vec<ChannelState>,
}

/// One channel's running state: the mark that waits, and the lamp that stays.
#[derive(Debug, Clone, Copy, Default)]
struct ChannelState {
    level: f32,
    peak: Ballistics,
    latch: ClipLatch,
}

/// **What a meter's level is a measurement of.**
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Peak {
    /// The largest sample: what the engine publishes for an audio bus and what
    /// the `Meter` UGen reads. The default, as it is in every meter.
    #[default]
    Sample,
    /// The peak of the reconstructed signal between the samples, in dBTP, as a
    /// `TruePeak` UGen writes it -- read against -1 dBTP rather than full scale.
    True,
}

impl Peak {
    fn parse(name: &str) -> Option<Peak> {
        match name {
            "sample" => Some(Peak::Sample),
            "true" => Some(Peak::True),
            _ => None,
        }
    }
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The axis the props describe. A **level is an amplitude**, so an audio bus is
/// read in decibels unless the widget was given a range of its own; a control
/// bus carries whatever the script put there, so it is read over `min..max`
/// unless the widget says `scale: "db"`. Stating `scale` settles it either way.
fn axis_of(props: &Map<String, Value>, rate: Rate) -> MeterAxis {
    let ranged = props.contains_key("min") || props.contains_key("max");
    let decibels = match props.get("scale").and_then(Value::as_str) {
        Some("db") | Some("decibels") => true,
        Some(_) => false,
        None => rate == Rate::Audio && !ranged,
    };
    if decibels {
        MeterAxis::Decibels {
            floor_db: floor_of(props),
        }
    } else {
        MeterAxis::Linear {
            min: parse::number(props, "min", 0.0),
            max: parse::number(props, "max", 1.0),
        }
    }
}

/// Where the scale bottoms out: `floor_db` as stated, or the dynamic range of
/// a resolution (`bits`), or the strip a mix is read on.
fn floor_of(props: &Map<String, Value>) -> f32 {
    if let Some(db) = parse::opt_number(props, "floor_db") {
        return db.min(-1.0);
    }
    match props.get("bits").and_then(Value::as_u64) {
        Some(bits) => floor_db_for_bits(bits as u32),
        None => clausters_core::measure::METER_FLOOR_DB,
    }
}

/// The props a `meter` node carries, read once -- shared by the constructor and
/// by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Meter {
    let rate = Rate::parse(props.get("rate").and_then(Value::as_str));
    let axis = axis_of(props, rate);
    let channels = (parse::int_prop(props, "channels", 1).max(1) as usize).min(MAX_CHANNELS);
    let mut meter = Meter {
        bus: parse::int_prop(props, "bus", 0),
        rate,
        channels,
        axis,
        hold: parse::number(props, "hold", HOLD).max(0.0),
        decay: parse::number(props, "decay", clausters_core::measure::METER_FALL_DB).max(0.0),
        clip: props.get("clip").and_then(Value::as_i64).map(|b| b as i32),
        ruler: ruler_of(props, axis),
        readout: props.get("readout").and_then(parse::truthy).unwrap_or(true),
        peak: props
            .get("peak")
            .and_then(Value::as_str)
            .and_then(Peak::parse)
            .unwrap_or_default(),
        zones: props
            .get("zones")
            .and_then(Zones::parse)
            .unwrap_or_default(),
        label: parse::label(props),
        state: vec![ChannelState::default(); channels],
    };
    meter.resize();
    meter
}

/// Which side the numbers fall on. A meter drawn in decibels carries them
/// unless told not to -- a scale nobody can read is a bar -- and one drawn over a
/// plain range carries none, there being no ladder to draw.
fn ruler_of(props: &Map<String, Value>, axis: MeterAxis) -> Option<Side> {
    let default = matches!(axis, MeterAxis::Decibels { .. }).then_some(Side::Left);
    match props.get("ruler") {
        None => default,
        Some(v) => side_of(v, default),
    }
}

fn side_of(v: &Value, default: Option<Side>) -> Option<Side> {
    match v.as_str() {
        Some("left") => Some(Side::Left),
        Some("right") => Some(Side::Right),
        Some("off") | Some("none") => None,
        Some(_) => default,
        None => match parse::truthy(v) {
            Some(false) => None,
            Some(true) => default.or(Some(Side::Left)),
            None => default,
        },
    }
}

impl Meter {
    /// Keeps one state per channel, so a `/gui_set channels` neither drops a
    /// column's mark nor reads past the end of the table.
    fn resize(&mut self) {
        self.state.resize(self.channels, ChannelState::default());
    }

    /// Where this meter's top is, in the units it reads: full scale on a
    /// decibel axis, the top of the range on a plain one. What an over is
    /// measured against.
    fn ceiling(&self) -> f32 {
        match self.axis {
            // A true-peak reading is read against the ceiling a true-peak
            // reading has: -1 dBTP, EBU R128's and every delivery
            // specification's, since a converter, a rate conversion and an
            // encoder downstream each move the peak by a fraction of a decibel.
            MeterAxis::Decibels { .. } if self.reads_true_peak() => {
                clausters_core::measure::amplitude_of_db(
                    clausters_core::resample::TRUE_PEAK_CEILING_DBTP,
                )
            }
            MeterAxis::Decibels { .. } => CLIP_CEILING,
            MeterAxis::Linear { max, .. } => max,
        }
    }

    /// **Whether this meter reads a true peak.** Only a control bus can carry
    /// one: at audio rate the level is the one the engine publishes, which is
    /// the block's largest *sample*, and calling that a true peak would read
    /// the lamp against a ceiling the number never meant. So `peak: "true"`
    /// takes effect with `rate: "control"` over a `TruePeak` bus, and an audio
    /// meter stays a sample meter whatever it was told.
    fn reads_true_peak(&self) -> bool {
        self.peak == Peak::True && self.rate == Rate::Control
    }

    /// Puts every lamp out -- the hand's verb, and the only one the widget has.
    fn clear(&mut self) {
        for ch in &mut self.state {
            ch.latch.clear();
        }
    }

    fn reads(&self) -> Vec<ChannelRead> {
        self.state
            .iter()
            .map(|ch| ChannelRead {
                level: ch.level,
                // A hold of zero is a meter with no mark, rather than a mark
                // sitting on the column it is supposed to be read against.
                mark: if self.hold > 0.0 {
                    ch.peak.level()
                } else {
                    0.0
                },
                clipped: ch.latch.lit().then(|| ch.latch.max()),
            })
            .collect()
    }
}

impl Element for Meter {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "bus" => v.as_i64().map(|n| self.bus = n as i32).is_some(),
            "rate" => parse::set_rate(&mut self.rate, v),
            "channels" => match v.as_i64() {
                Some(n) => {
                    self.channels = (n.max(1) as usize).min(MAX_CHANNELS);
                    self.resize();
                    true
                }
                None => false,
            },
            "min" | "max" | "scale" | "floor_db" | "bits" => {
                let mut props = Map::new();
                props.insert(key.to_string(), v.clone());
                // The axis is read from the three props together, so one of
                // them arriving alone is read against what the others are now.
                let (min, max) = match self.axis {
                    MeterAxis::Linear { min, max } => (min, max),
                    MeterAxis::Decibels { floor_db } => {
                        props.entry("floor_db").or_insert(Value::from(floor_db));
                        (0.0, 1.0)
                    }
                };
                props.entry("min").or_insert(Value::from(min));
                props.entry("max").or_insert(Value::from(max));
                if key != "scale" {
                    props.insert(
                        "scale".into(),
                        Value::from(match self.axis {
                            MeterAxis::Decibels { .. } => "db",
                            MeterAxis::Linear { .. } => "linear",
                        }),
                    );
                }
                self.axis = axis_of(&props, self.rate);
                true
            }
            "hold" => parse::set_f(&mut self.hold, v),
            "decay" => parse::set_f(&mut self.decay, v),
            "clip" => {
                self.clip = v.as_i64().map(|b| b as i32).filter(|b| *b >= 0);
                true
            }
            "ruler" => {
                self.ruler = side_of(v, self.ruler);
                true
            }
            "peak" => match v.as_str().and_then(Peak::parse) {
                Some(p) => {
                    self.peak = p;
                    true
                }
                None => false,
            },
            "readout" => match parse::truthy(v) {
                Some(b) => {
                    self.readout = b;
                    true
                }
                None => false,
            },
            "zones" => match Zones::parse(v) {
                Some(zones) => {
                    self.zones = zones;
                    true
                }
                None => false,
            },
            "label" => parse::set_label(&mut self.label, v),
            _ => false,
        }
    }

    /// One tick advances what a meter *keeps*: the level it shows, the mark
    /// that waits and the lamp that stays. All three are time-based, so they
    /// belong to the tick and not to the repaint -- a window that redraws twice
    /// must not let a peak fall twice.
    fn tick(&mut self, live: &Live) {
        let ceiling = self.ceiling();
        let (decay, hold) = (self.decay, self.hold);
        for (i, ch) in self.state.iter_mut().enumerate() {
            let bus = self.bus + i as i32;
            ch.level = live.level(bus, self.rate);
            ch.peak
                .tick(ch.level, live.dt as f32, decay, hold.max(f32::EPSILON));
            let overs = self.clip.map(|first| live.control(first + i as i32));
            ch.latch.tick(ch.level, overs, ceiling);
        }
    }

    /// **A meter is thin, and says so.** It asks for one narrow column per
    /// channel (plus the ladder's strip when it carries one) and stays elastic
    /// on the height, which is the shape of the thing: a level is read by how
    /// far up it goes, so a meter given more width spends it on nothing while a
    /// meter given more height reads better. A widget that wants a wider one
    /// says `w`, the placement prop every widget has.
    fn natural(&self, m: &crate::host::metrics::Metrics, scale: f32) -> Natural {
        let column = (m.box_side * 0.5).max(3.0);
        let columns =
            self.channels as f32 * column + (self.channels.saturating_sub(1)) as f32 * m.divider_w;
        let ladder = if self.ruler.is_some() { m.ruler_w } else { 0.0 };
        (
            Some(crate::host::metrics::snap_px(
                columns + ladder + m.pad,
                scale,
            )),
            None,
        )
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let reads = self.reads();
        meters::draw_meter_view(
            d,
            ctx.rect,
            &MeterView {
                channels: &reads,
                axis: self.axis,
                readout: self.readout,
                label: self.label.as_deref(),
                ruler: self.ruler,
                zones: &self.zones,
            },
        );
    }

    /// **A click puts the lamps out.** The one thing a hand does to a meter,
    /// and the reason the mark is latched at all: a reader clears it when they
    /// have seen it, and what happens after that is news. It reports nothing --
    /// the latch is the reader's state, not the multitrack's -- so a window full of
    /// meters is silent on the wire however much it is clicked.
    fn press(&mut self, _at: (f64, f64), _input: &Input) -> Claim {
        self.clear();
        Claim::take()
    }

    fn needs(&self) -> Needs {
        // The rate picks the table, so it picks the declaration: samples are
        // never recorded for a meter, at either rate. The clip counts are
        // control buses whatever the level's rate is -- they are a count, and a
        // count is a value.
        let level: Vec<i32> = (0..self.channels as i32).map(|i| self.bus + i).collect();
        let counts: Vec<i32> = self
            .clip
            .map(|first| (0..self.channels as i32).map(|i| first + i).collect())
            .unwrap_or_default();
        match self.rate {
            Rate::Audio => Needs {
                levels: level,
                buses: counts,
                ..Default::default()
            },
            Rate::Control => Needs {
                buses: level.into_iter().chain(counts).collect(),
                ..Default::default()
            },
        }
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::BusSource;
    use crate::host::layout::Rect;
    use crate::host::metrics::Metrics;
    use crate::host::widget::element::Mods;
    use std::collections::HashMap;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    /// **`zones` is a prop and a `/gui_set`.** Read at build, replaced by a
    /// set that parses, and a set that does not is refused and leaves what
    /// was there.
    #[test]
    fn zones_are_read_and_set() {
        let mut m = from_props(&props(r#"{"rate":"control","min":-1,"max":1,"zones":1}"#));
        assert_eq!(m.zones, Zones::On);
        assert!(m.set("zones", &Value::String(r#"[[0,"meter_mid"]]"#.into())));
        assert!(matches!(m.zones, Zones::Stops(ref s) if s.len() == 1));
        assert!(!m.set("zones", &Value::String("[[0, 7]]".into())));
        assert!(
            matches!(m.zones, Zones::Stops(_)),
            "a refused set keeps the zones"
        );
        assert_eq!(from_props(&props("{}")).zones, Zones::Auto);
    }

    /// A source whose two tables answer differently, so a read that went to the
    /// wrong one is visible rather than plausible.
    struct Buses;

    impl BusSource for Buses {
        fn control(&self, index: usize) -> f32 {
            index as f32 / 10.0
        }

        fn level(&self, bus: i32) -> f32 {
            bus as f32 / 100.0
        }
    }

    /// A source answering one fixed level on every bus and one fixed count.
    struct Fixed(f32, f32);

    impl BusSource for Fixed {
        fn control(&self, _index: usize) -> f32 {
            self.1
        }

        fn level(&self, _bus: i32) -> f32 {
            self.0
        }
    }

    fn tick(meter: &mut Meter, source: &dyn BusSource, dt: f64) {
        let histories = HashMap::new();
        meter.tick(&Live {
            bus: Some(source),
            sample_rate: 48_000.0,
            dt,
            histories: &histories,
        });
    }

    #[test]
    fn props_parse_and_default() {
        let m = from_props(&props(
            r#"{"bus":4,"rate":"audio","channels":2,"hold":0.5,"decay":12,
                "clip":30,"ruler":"right","label":"out"}"#,
        ));
        assert_eq!((m.bus, m.rate, m.channels), (4, Rate::Audio, 2));
        assert_eq!((m.hold, m.decay, m.clip), (0.5, 12.0, Some(30)));
        assert_eq!(m.ruler, Some(Side::Right));
        assert_eq!(m.label.as_deref(), Some("out"));

        let m = from_props(&props("{}"));
        assert_eq!(
            (m.bus, m.rate, m.channels),
            (0, Rate::Audio, 1),
            "a meter watches one audio bus unless told"
        );
        assert_eq!(m.clip, None);
        assert_eq!(m.hold, HOLD);
        assert_eq!(m.decay, clausters_core::measure::METER_FALL_DB);
    }

    /// **A level is an amplitude**: an audio bus is read in decibels, a control
    /// bus over its range, and `scale` settles it either way.
    #[test]
    fn the_axis_follows_what_the_bus_carries() {
        let db = |json: &str| matches!(from_props(&props(json)).axis, MeterAxis::Decibels { .. });
        assert!(db("{}"), "an audio level is read in decibels");
        assert!(!db(r#"{"rate":"control"}"#), "a control bus carries values");
        assert!(
            !db(r#"{"min":0,"max":2}"#),
            "a range of its own is a range, not a floor"
        );
        assert!(db(r#"{"rate":"control","scale":"db"}"#));
        assert!(!db(r#"{"scale":"linear"}"#));
    }

    /// The floor is the reader's question: the mixing strip, a stated number,
    /// or the dynamic range of the resolution the multitrack is rendered at.
    #[test]
    fn the_floor_is_the_strip_a_number_or_a_resolution() {
        let floor = |json: &str| match from_props(&props(json)).axis {
            MeterAxis::Decibels { floor_db } => floor_db,
            MeterAxis::Linear { .. } => panic!("expected a decibel axis"),
        };
        assert_eq!(floor("{}"), clausters_core::measure::METER_FLOOR_DB);
        assert_eq!(floor(r#"{"floor_db":-90}"#), -90.0);
        assert!((floor(r#"{"bits":16}"#) + 96.3).abs() < 0.1);
        assert!((floor(r#"{"bits":24}"#) + 144.5).abs() < 0.1);
        assert_eq!(
            floor(r#"{"bits":16,"floor_db":-40}"#),
            -40.0,
            "a stated floor wins over a resolution's"
        );
    }

    #[test]
    fn a_set_lands_on_its_own_key_and_declines_the_rest() {
        let mut m = from_props(&props("{}"));
        assert!(m.set("bus", &Value::from(7)));
        assert!(m.set("rate", &Value::from("control")));
        assert!(m.set("channels", &Value::from(3)));
        assert!(m.set("hold", &Value::from(0.25)));
        assert!(m.set("label", &Value::from("in")));
        assert_eq!(
            (m.bus, m.rate, m.channels, m.hold),
            (7, Rate::Control, 3, 0.25)
        );
        assert_eq!(m.state.len(), 3, "a column per channel, with its own mark");
        assert!(!m.set("nonesuch", &Value::from(1)));
    }

    /// A live `floor_db` moves the floor and leaves the axis a decibel axis;
    /// a live `min` makes it a plain range. The three props are read together,
    /// so one arriving alone is read against what the others now are.
    #[test]
    fn the_axis_can_be_set_live() {
        let mut m = from_props(&props("{}"));
        assert!(m.set("floor_db", &Value::from(-96.0)));
        assert!(matches!(m.axis, MeterAxis::Decibels { floor_db } if floor_db == -96.0));
        assert!(m.set("scale", &Value::from("linear")));
        assert!(matches!(m.axis, MeterAxis::Linear { min: 0.0, max: 1.0 }));
        assert!(m.set("max", &Value::from(4.0)));
        assert!(matches!(m.axis, MeterAxis::Linear { max, .. } if max == 4.0));
    }

    /// The declaration *is* the rate: a control-rate meter asks for its buses
    /// to be streamed, an audio-rate one asks for published levels, and neither
    /// ever asks the server to record samples. A clip count is a control bus at
    /// either rate -- it is a count, and a count is a value.
    #[test]
    fn the_rate_picks_what_is_declared() {
        let control = from_props(&props(r#"{"bus":3,"rate":"control","channels":2}"#)).needs();
        assert_eq!(control.buses, vec![3, 4]);
        assert!(control.levels.is_empty() && control.taps.is_empty());

        let audio = from_props(&props(r#"{"bus":3,"channels":2,"clip":20}"#)).needs();
        assert_eq!(audio.levels, vec![3, 4]);
        assert_eq!(audio.buses, vec![20, 21]);
        assert!(audio.taps.is_empty());
    }

    /// And the rate picks the table the *tick* reads, which is the same
    /// question asked of the source instead of of a collector.
    #[test]
    fn the_rate_picks_the_table_read() {
        let read = |json: &str| {
            let mut m = from_props(&props(json));
            tick(&mut m, &Buses, 0.0);
            m.state[0].level
        };
        assert_eq!(read(r#"{"bus":5,"rate":"control"}"#), 0.5);
        assert_eq!(read(r#"{"bus":5}"#), 0.05);
    }

    /// The mark holds where the level was, then falls at the declared rate --
    /// the half of a meter that waits to be read.
    #[test]
    fn the_mark_holds_and_then_falls() {
        let mut m = from_props(&props(r#"{"hold":1.0,"decay":20}"#));
        tick(&mut m, &Fixed(0.5, 0.0), 0.0);
        assert_eq!(m.state[0].peak.level(), 0.5, "the attack is instantaneous");
        tick(&mut m, &Fixed(0.0, 0.0), 0.5);
        assert_eq!(m.state[0].peak.level(), 0.5, "still held");
        tick(&mut m, &Fixed(0.0, 0.0), 1.0);
        let fallen = m.state[0].peak.level();
        assert!(fallen < 0.5 && fallen > 0.0, "and then falls: {fallen}");
        // A hold of zero is a meter with no mark at all, rather than a mark
        // sitting on the column it is read against.
        let mut none = from_props(&props(r#"{"hold":0}"#));
        tick(&mut none, &Fixed(0.5, 0.0), 0.0);
        assert_eq!(none.reads()[0].mark, 0.0);
    }

    /// The lamp stays lit after the over has passed, and a click is what puts
    /// it out.
    #[test]
    fn the_lamp_latches_and_a_click_clears_it() {
        let mut m = from_props(&props("{}"));
        tick(&mut m, &Fixed(0.5, 0.0), 0.1);
        assert!(m.reads()[0].clipped.is_none());
        tick(&mut m, &Fixed(1.2, 0.0), 0.1);
        tick(&mut m, &Fixed(0.1, 0.0), 0.1);
        let read = m.reads()[0];
        assert!(read.clipped.is_some(), "the over is over; the lamp is not");
        assert!(
            (read.clipped.unwrap() - 1.2).abs() < 1e-6,
            "and by how much"
        );
        let metrics = Metrics::default();
        let input = Input {
            metrics: &metrics,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 40.0, 120.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            clicks: 1,
            time: None,
        };
        assert_eq!(m.press((0.0, 0.0), &input), Claim::take());
        assert!(m.reads()[0].clipped.is_none());
    }

    /// **A meter is thin**: it asks for its columns and its ladder and nothing
    /// else, and stays elastic on the height. A wider one is asked for with
    /// `w`, which is the placement prop every widget has.
    #[test]
    fn a_meter_asks_for_a_narrow_column_and_no_height() {
        let m = Metrics::default();
        let one = from_props(&props(r#"{"ruler":false}"#)).natural(&m, 1.0);
        let two = from_props(&props(r#"{"channels":2,"ruler":false}"#)).natural(&m, 1.0);
        let (Some(one_w), Some(two_w)) = (one.0, two.0) else {
            panic!("a meter states its width");
        };
        assert!(
            one.1.is_none() && two.1.is_none(),
            "the height is the cell's"
        );
        assert!(one_w < m.header_w, "narrower than a name: {one_w}");
        assert!(two_w > one_w, "a second channel is a second column");
        let ruled = from_props(&props("{}")).natural(&m, 1.0).0.unwrap();
        assert!(ruled > one_w, "the ladder asks for its strip too");
    }

    /// The numbers are a prop: a strip of meters in a header carries no ladder
    /// and no figures, which is a meter and not a degraded one.
    #[test]
    fn the_lamps_number_can_be_turned_off() {
        assert!(from_props(&props("{}")).readout, "on unless told");
        let mut m = from_props(&props(r#"{"readout":false}"#));
        assert!(!m.readout);
        assert!(m.set("readout", &Value::from(1)));
        assert!(m.readout);
        assert!(!m.set("readout", &Value::from("yes")), "a flag is a flag");
    }

    /// **A meter reads the sample peak unless told otherwise**, and a true peak
    /// is read against -1 dBTP: a level of -0.5 dBTP lights the lamp that a
    /// sample reading of the same number would leave dark.
    #[test]
    fn a_true_peak_meter_lights_at_minus_one_dbtp() {
        let near = clausters_core::measure::amplitude_of_db(-0.5);
        assert_eq!(
            from_props(&props("{}")).peak,
            Peak::Sample,
            "sample by default"
        );

        // At control rate the level is the control bus's value, which is the
        // second field of the fixed source.
        let mut sample = from_props(&props(r#"{"rate":"control","scale":"db"}"#));
        tick(&mut sample, &Fixed(0.0, near), 0.1);
        assert!(sample.reads().iter().all(|c| c.clipped.is_none()));

        let mut truth = from_props(&props(r#"{"rate":"control","scale":"db","peak":"true"}"#));
        tick(&mut truth, &Fixed(0.0, near), 0.1);
        assert!(
            truth.reads()[0].clipped.is_some(),
            "over the true-peak ceiling"
        );
    }

    /// **An audio meter is a sample meter whatever it is told**, because the
    /// level the engine publishes is the largest sample and nothing else.
    #[test]
    fn an_audio_meter_cannot_be_told_it_reads_a_true_peak() {
        let near = clausters_core::measure::amplitude_of_db(-0.5);
        let mut m = from_props(&props(r#"{"peak":"true"}"#));
        assert!(!m.reads_true_peak());
        tick(&mut m, &Fixed(near, 0.0), 0.1);
        assert!(m.reads()[0].clipped.is_none());
        assert!(m.set("peak", &Value::from("sample")));
        assert!(
            !m.set("peak", &Value::from("loud")),
            "an unknown word is declined"
        );
    }

    /// With a counting bus the lamp follows the count and not the level, which
    /// is what makes a **run** at full scale distinguishable from a sample that
    /// merely touched it.
    #[test]
    fn a_counting_bus_is_what_lights_it() {
        let mut m = from_props(&props(r#"{"clip":9}"#));
        // A level at full scale with the count standing still lights nothing.
        tick(&mut m, &Fixed(1.0, 4.0), 0.1);
        tick(&mut m, &Fixed(1.0, 4.0), 0.1);
        assert!(m.reads()[0].clipped.is_none());
        tick(&mut m, &Fixed(0.3, 5.0), 0.1);
        assert!(m.reads()[0].clipped.is_some(), "the count moved");
        // And a count that starts again is a new pass, which clears it.
        tick(&mut m, &Fixed(0.3, 0.0), 0.1);
        assert!(m.reads()[0].clipped.is_none());
    }
}
